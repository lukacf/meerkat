#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
//! Item 9 - pinned golden envelopes for the shared six-verb labels domain.
//!
//! The stdio JSON-RPC dispatcher (`src/rpc.rs` -> `src/rpc/mob_methods.rs`)
//! and HTTP console dispatcher (`src/http_console.rs`) both delegate label
//! semantics to `runtime::dispatch_label_method`. This file pins that shared
//! authority and the intentionally transport-specific envelopes around it.
//!
//! The six verbs:
//!
//! | method                      | semantic handler                        |
//! |-----------------------------|-----------------------------------------|
//! | `meta/mob_labels/set`     | `runtime::dispatch_label_method`        |
//! | `meta/mob_labels/get`     | `runtime::dispatch_label_method`        |
//! | `meta/mob_labels/delete`  | `runtime::dispatch_label_method`        |
//! | `meta/run_labels/set`     | `runtime::dispatch_label_method`        |
//! | `meta/run_labels/get`     | `runtime::dispatch_label_method`        |
//! | `meta/run_labels/delete`  | `runtime::dispatch_label_method`        |
//!
//! Both project the same `crate::runtime::LabelRpcResult` from the same
//! `RuntimeMetadataTable`, so the *payloads* already agree. What does NOT
//! agree - and what a naive merge would silently flatten - is:
//!
//! 1. **The access decision.** The HTTP console runs two gates before
//!    dispatch: `is_console_mutating_rpc_method` (console read-only) and
//!    `console_rpc_access_requirements` (ABAC). Both are *fail-open lookup
//!    tables*: an unlisted method is non-mutating and permitted under
//!    `console.read_only`; an unmapped method carries no grant requirement.
//!    The stdio dispatcher runs neither gate. A golden that pinned only the
//!    happy path would give false confidence to exactly the merge it exists
//!    to protect, so every verb here has its access decision pinned in both
//!    directions: denied when the grant is absent, permitted when present.
//! 2. **The error message prefix.** stdio's `label_response` renders
//!    `format!("Invalid params: {message}")`; the console's `invalid_params`
//!    renders the bare message. Same code (-32602), different text.
//! 3. **The wire field order.** stdio serializes the `JsonRpcResponse`
//!    struct directly (declaration order: jsonrpc, id, result, error); the
//!    console converts it to a `serde_json::Value` first, and `serde_json`
//!    is built here *without* `preserve_order`, so `Map` is a `BTreeMap` and
//!    the console emits keys lexicographically. Re-parsing hides this - only
//!    the raw bytes show it.
//! 4. **Notifications.** A request with no `id` gets an empty string from
//!    stdio and a full `id: null` envelope from the console.
//! 5. **The absent-table branch.** The console can be built with
//!    `metadata_table: None` and answers -32602; the stdio runtime always
//!    owns a table and structurally cannot reach that branch.
//!
//! Every one of those is asserted below. A change to the shared handler or
//! either transport projection therefore fails at the exact contract seam.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_meta::runtime::{LabelRpcResult, dispatch_label_method};
use meerkat_meta::{
    AccessControlConfig, AccessController, AccessRule, AuthPolicy, AuthProvider, BigQueryNaming,
    ConsolePolicy, DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
    RuntimeDecisionInputs, RuntimeOpsPolicy, TrustedOidcRuntimeConfig, UnifiedRuntime,
    build_runtime_decision_state, console_json_router_with_runtime, handle_unified_rpc_json,
};
use meerkat_mob::{MobDefinition, MobStorage};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

/// Per-test mob id counter: 0.8.23's fail-closed in-proc registration
/// means concurrently running tests must not share a supervisor route.
static NEXT_TEST_MOB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

// ---------------------------------------------------------------------------
// The domain under pin
// ---------------------------------------------------------------------------

/// Every label verb, in the order the two dispatchers list them.
const ALL_LABEL_VERBS: &[&str] = &[
    "meta/mob_labels/set",
    "meta/mob_labels/get",
    "meta/mob_labels/delete",
    "meta/run_labels/set",
    "meta/run_labels/get",
    "meta/run_labels/delete",
];

/// The four verbs `is_console_mutating_rpc_method` currently lists. If a
/// merge drops one of these from the table it becomes silently callable on a
/// read-only console.
const MUTATING_LABEL_VERBS: &[&str] = &[
    "meta/mob_labels/set",
    "meta/mob_labels/delete",
    "meta/run_labels/set",
    "meta/run_labels/delete",
];

/// The two verbs deliberately absent from the mutating table.
const READ_LABEL_VERBS: &[&str] = &["meta/mob_labels/get", "meta/run_labels/get"];

/// The single ABAC action all six verbs map to, resource-less.
///
/// Pinned as a literal rather than as `access::ACTION_RUNTIME_ADMIN`
/// because the *string* is what reaches the wire (`data.action`, the
/// `access denied: ...` message) and what operator `access.toml` rules are
/// written against - a golden written in terms of the constant would follow
/// a value change silently. The constant is public
/// (`meerkat_meta::access::ACTION_RUNTIME_ADMIN`, re-exported from
/// `src/access/model.rs`), so the two are tied together explicitly in
/// `console_abac_grant_of_runtime_admin_alone_permits_all_six_label_verbs`.
const LABEL_GRANT_ACTION: &str = "runtime.admin";

const RUN_ID: &str = "run-golden";

/// Minimal valid params per verb: run-scoped verbs need `run_id`, `set`
/// needs `labels`.
fn params_for(method: &str) -> Value {
    match method {
        "meta/mob_labels/set" => json!({ "labels": { "env": "dev" } }),
        "meta/mob_labels/get" | "meta/mob_labels/delete" => json!({}),
        "meta/run_labels/set" => json!({ "run_id": RUN_ID, "labels": { "env": "dev" } }),
        "meta/run_labels/get" | "meta/run_labels/delete" => json!({ "run_id": RUN_ID }),
        other => panic!("not a label verb: {other}"),
    }
}

/// `set` and `delete` answer `{"accepted": true}`; `get` answers
/// `{"labels": {...}}`. Both dispatchers build these from the same
/// `LabelRpcResult`.
fn expected_result_for(method: &str, labels: Value) -> Value {
    if method.ends_with("/get") {
        json!({ "labels": labels })
    } else {
        json!({ "accepted": true })
    }
}

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

fn trusted_toml() -> String {
    r#"
[[modules]]
id = "router"
command = "router-bin"
args = []
restart_policy = "always"
"#
    .to_string()
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.local","jwks_uri":"https://trusted.meta.local/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"}]}"#
            .to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

/// `require_app_auth: false` keeps the console on the anonymous principal so
/// every response below is a dispatcher envelope rather than a 401. Note
/// `ConsolePolicy::default()` sets `require_app_auth: true` - leaving it
/// alone would swap the entire envelope shape.
fn decision_state(read_only: bool) -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "label_golden_dataset".to_string(),
            table: "label_golden_table".to_string(),
        },
        trusted_meta_toml: trusted_toml(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec!["root@example.test".to_string()],
        },
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth: false,
            read_only,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: include_str!("../assets/release-targets.json").to_string(),
    })
    .expect("decision state builds")
}

struct Fixture {
    _temp_dir: TempDir,
    runtime: UnifiedRuntime,
}

impl Fixture {
    /// Build the HTTP console router bound to this runtime. Must be called
    /// *after* `set_access_controller`: the router snapshots the controller
    /// (and the metadata table) at construction time.
    fn console(&self, read_only: bool) -> axum::Router {
        self.runtime
            .build_console_json_router(decision_state(read_only))
    }

    async fn shutdown(self) {
        let report = self.runtime.shutdown().await;
        assert!(
            report.mob_stop.is_ok(),
            "mob stop failed at teardown: {:?}",
            report.mob_stop
        );
    }
}

async fn label_fixture() -> Fixture {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let session_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");

    let factory = AgentFactory::new(&session_path).comms(true);
    let session_service = Arc::new(build_ephemeral_service(factory, Config::default(), 16));

    let definition = MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "label-golden-mob-{}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#,
        NEXT_TEST_MOB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
            namespace: "label-envelope-golden".to_string(),
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

/// Enabled ABAC granting the anonymous principal one unscoped action that is
/// *not* `runtime.admin`. Any label verb reaching the ABAC gate must be
/// denied against this config; if `console_rpc_access_requirements` ever
/// stops mapping a label verb, the fail-open `_ => None` arm lets it through
/// and the denial test fails.
fn deny_runtime_admin_config() -> AccessControlConfig {
    AccessControlConfig {
        approvers: Default::default(),
        enabled: true,
        // Non-empty admins is a config invariant (`EnabledWithoutAdmins`);
        // this subject is never authenticated in these tests.
        admins: vec!["root@example.test".to_string()],
        groups: BTreeMap::new(),
        rules: vec![AccessRule {
            id: "anonymous-observes-the-mob".to_string(),
            // Unscoped grant of an unrelated action: proves the denial below
            // is about the *specific* action, not about "no rules at all".
            actions: vec!["mob.observe".to_string()],
            ..AccessRule::default()
        }],
    }
}

/// Enabled ABAC granting the anonymous principal exactly one unscoped
/// action: `runtime.admin`. No `subjects`/`groups` selector so the rule
/// matches the anonymous principal; no `agents`/`roles`/`match_labels`
/// selector so it matches `AccessResource::none()`, which is what the
/// resource-less `view.allows(action)` check needs.
fn grant_runtime_admin_config() -> AccessControlConfig {
    AccessControlConfig {
        approvers: Default::default(),
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        groups: BTreeMap::new(),
        rules: vec![AccessRule {
            id: "anonymous-runtime-admin".to_string(),
            actions: vec![LABEL_GRANT_ACTION.to_string()],
            ..AccessRule::default()
        }],
    }
}

// ---------------------------------------------------------------------------
// Transport drivers
// ---------------------------------------------------------------------------

const REQUEST_ID: &str = "golden";

fn request_payload(method: &str, params: &Value) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": REQUEST_ID,
        "method": method,
        "params": params,
    })
    .to_string()
}

/// Raw stdio response bytes (a JSON string, or `""` for a notification).
async fn stdio_raw(runtime: &UnifiedRuntime, method: &str, params: Value) -> String {
    let payload = request_payload(method, &params);
    handle_unified_rpc_json(runtime, &payload, Duration::from_secs(5), None, None).await
}

async fn stdio(runtime: &UnifiedRuntime, method: &str, params: Value) -> Value {
    let raw = stdio_raw(runtime, method, params).await;
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("stdio json for {method}: {err}: {raw}"))
}

/// Raw HTTP console response body. The status is always 200 for a dispatched
/// JSON-RPC call - errors ride inside the envelope, not the HTTP status.
async fn http_raw(app: &axum::Router, method: &str, params: Value) -> String {
    let payload = request_payload(method, &params);
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(payload))
                .expect("console rpc request"),
        )
        .await
        .expect("console rpc response");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "console rpc must answer 200 and carry errors in the envelope ({method})"
    );
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("console rpc body");
    String::from_utf8(body.to_vec()).expect("utf8 console rpc body")
}

async fn http(app: &axum::Router, method: &str, params: Value) -> Value {
    let raw = http_raw(app, method, params).await;
    serde_json::from_str(&raw).unwrap_or_else(|err| panic!("http json for {method}: {err}: {raw}"))
}

// ---------------------------------------------------------------------------
// Envelope assertions
// ---------------------------------------------------------------------------

/// Keys of a JSON object. Re-parsing normalises order (serde_json `Map` is a
/// `BTreeMap` here), so this pins the key *set*, never the wire order.
fn key_set(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|map| map.keys().cloned().collect::<Vec<String>>())
        .unwrap_or_else(|| panic!("expected a json object, got: {value}"))
}

/// A success envelope is exactly `{jsonrpc, id, result}` - no `error` key at
/// all (both dispatchers skip `None` fields).
fn assert_success_envelope(label: &str, envelope: &Value, expected_result: &Value) {
    assert_eq!(
        key_set(envelope),
        vec![
            "id".to_string(),
            "jsonrpc".to_string(),
            "result".to_string()
        ],
        "{label}: success envelope must carry exactly jsonrpc/id/result: {envelope:#?}"
    );
    assert_eq!(envelope["jsonrpc"], json!("2.0"), "{label}: {envelope:#?}");
    assert_eq!(
        envelope["id"],
        json!(REQUEST_ID),
        "{label}: id must echo the request: {envelope:#?}"
    );
    assert_eq!(
        &envelope["result"], expected_result,
        "{label}: result payload: {envelope:#?}"
    );
}

/// An error envelope is exactly `{jsonrpc, id, error}`. `expected_data`
/// of `None` pins that the `error` object has *no* `data` key (the -32602
/// label errors), which is itself a contract difference from the gate errors
/// that do carry one.
fn assert_error_envelope(
    label: &str,
    envelope: &Value,
    code: i64,
    message: &str,
    expected_data: Option<&Value>,
) {
    assert_eq!(
        key_set(envelope),
        vec!["error".to_string(), "id".to_string(), "jsonrpc".to_string()],
        "{label}: error envelope must carry exactly jsonrpc/id/error: {envelope:#?}"
    );
    assert_eq!(envelope["jsonrpc"], json!("2.0"), "{label}: {envelope:#?}");
    assert_eq!(
        envelope["id"],
        json!(REQUEST_ID),
        "{label}: id must echo the request: {envelope:#?}"
    );
    assert_eq!(
        envelope["error"]["code"],
        json!(code),
        "{label}: error code: {envelope:#?}"
    );
    assert_eq!(
        envelope["error"]["message"],
        json!(message),
        "{label}: error message: {envelope:#?}"
    );
    match expected_data {
        Some(data) => {
            assert_eq!(
                key_set(&envelope["error"]),
                vec![
                    "code".to_string(),
                    "data".to_string(),
                    "message".to_string()
                ],
                "{label}: error object must carry code/message/data: {envelope:#?}"
            );
            assert_eq!(
                &envelope["error"]["data"], data,
                "{label}: error data: {envelope:#?}"
            );
        }
        None => assert_eq!(
            key_set(&envelope["error"]),
            vec!["code".to_string(), "message".to_string()],
            "{label}: error object must carry exactly code/message - no data key: {envelope:#?}"
        ),
    }
}

/// The console read-only refusal: `-32010`, fixed message, `data.kind`.
fn assert_console_read_only(label: &str, envelope: &Value) {
    assert_error_envelope(
        label,
        envelope,
        -32010,
        "console is read-only",
        Some(&json!({ "kind": "read_only" })),
    );
}

/// The console ABAC refusal for a label verb. `resource` is pinned to `null`:
/// `console_rpc_access_requirements` computes a `target` from
/// `identity`/`member_id`/`agent_id`, but the label arm deliberately passes
/// `None`, so these are mob-wide checks and the denial must say so.
fn assert_label_access_denied(label: &str, envelope: &Value) {
    assert_error_envelope(
        label,
        envelope,
        -32030,
        &format!("access denied: {LABEL_GRANT_ACTION}"),
        Some(&json!({
            "kind": "access_denied",
            "action": LABEL_GRANT_ACTION,
            "resource": Value::Null,
        })),
    );
}

fn assert_no_error(label: &str, envelope: &Value) {
    assert!(
        envelope.get("error").is_none(),
        "{label} must succeed: {envelope:#?}"
    );
    assert!(
        envelope.get("result").is_some(),
        "{label} must carry a result: {envelope:#?}"
    );
}

// ===========================================================================
// 1. Payload goldens - the six verbs, both transports
// ===========================================================================

/// The full happy-path envelope for all six verbs on both transports,
/// against one shared runtime. `set` and `delete` answer
/// `{"accepted": true}`; `get` answers `{"labels": {...}}`.
#[tokio::test]
async fn label_verb_success_envelopes_are_pinned_on_both_transports() {
    let fixture = label_fixture().await;
    let console = fixture.console(false);

    for (transport, drive) in [("stdio", true), ("http", false)] {
        // Each transport gets a clean scope: delete first, then set/get.
        for method in ["meta/mob_labels/delete", "meta/run_labels/delete"] {
            let envelope = if drive {
                stdio(&fixture.runtime, method, params_for(method)).await
            } else {
                http(&console, method, params_for(method)).await
            };
            assert_success_envelope(
                &format!("{transport} {method} (pre-clean)"),
                &envelope,
                &json!({ "accepted": true }),
            );
        }

        // set -> {"accepted": true}
        for method in ["meta/mob_labels/set", "meta/run_labels/set"] {
            let params = if method.starts_with("meta/mob_labels") {
                json!({ "labels": { "repo": "agents", "env": "dev" } })
            } else {
                json!({
                    "run_id": RUN_ID,
                    "labels": { "trace_id": "alpha", "attempt": "2" },
                })
            };
            let envelope = if drive {
                stdio(&fixture.runtime, method, params).await
            } else {
                http(&console, method, params).await
            };
            assert_success_envelope(
                &format!("{transport} {method}"),
                &envelope,
                &json!({ "accepted": true }),
            );
        }

        // get -> {"labels": {...}}, keys lexicographic (labels_to_json_value
        // walks a BTreeMap, and serde_json's Map is a BTreeMap here).
        let mob_get = if drive {
            stdio(
                &fixture.runtime,
                "meta/mob_labels/get",
                params_for("meta/mob_labels/get"),
            )
            .await
        } else {
            http(
                &console,
                "meta/mob_labels/get",
                params_for("meta/mob_labels/get"),
            )
            .await
        };
        assert_success_envelope(
            &format!("{transport} meta/mob_labels/get"),
            &mob_get,
            &json!({ "labels": { "env": "dev", "repo": "agents" } }),
        );

        let run_get = if drive {
            stdio(
                &fixture.runtime,
                "meta/run_labels/get",
                params_for("meta/run_labels/get"),
            )
            .await
        } else {
            http(
                &console,
                "meta/run_labels/get",
                params_for("meta/run_labels/get"),
            )
            .await
        };
        assert_success_envelope(
            &format!("{transport} meta/run_labels/get"),
            &run_get,
            &json!({ "labels": { "attempt": "2", "trace_id": "alpha" } }),
        );

        // delete -> {"accepted": true}, and the scope reads back empty.
        for (delete_method, get_method) in [
            ("meta/mob_labels/delete", "meta/mob_labels/get"),
            ("meta/run_labels/delete", "meta/run_labels/get"),
        ] {
            let deleted = if drive {
                stdio(&fixture.runtime, delete_method, params_for(delete_method)).await
            } else {
                http(&console, delete_method, params_for(delete_method)).await
            };
            assert_success_envelope(
                &format!("{transport} {delete_method}"),
                &deleted,
                &json!({ "accepted": true }),
            );
            let after = if drive {
                stdio(&fixture.runtime, get_method, params_for(get_method)).await
            } else {
                http(&console, get_method, params_for(get_method)).await
            };
            assert_success_envelope(
                &format!("{transport} {get_method} after delete"),
                &after,
                &json!({ "labels": {} }),
            );
        }
    }

    fixture.shutdown().await;
}

/// The transport goldens above pin wire behavior. This test pins the shared
/// library handler underneath them directly, including mutation sensitivity:
/// each set/delete verb may affect only its own mob/run scope. If either
/// transport grows a private method switch again, its behavior can no longer
/// be explained by this single six-verb authority and the transport goldens
/// will diverge from these outcomes.
#[tokio::test]
async fn shared_label_domain_handler_is_the_six_verb_mutation_authority() {
    let table = meerkat_meta::RuntimeMetadataTable::new();
    let mob_id = "mob-direct";
    let mob_scope = meerkat_meta::MetadataScope::Mob(mob_id.to_string());
    let run_scope = meerkat_meta::MetadataScope::Run(mob_id.to_string(), RUN_ID.to_string());

    assert_eq!(
        dispatch_label_method(
            &table,
            mob_id,
            "meta/mob_labels/set",
            &json!({ "labels": { "scope": "mob" } }),
        )
        .await,
        Some(LabelRpcResult::Accepted),
    );
    assert_eq!(
        dispatch_label_method(
            &table,
            mob_id,
            "meta/run_labels/set",
            &json!({ "run_id": RUN_ID, "labels": { "scope": "run" } }),
        )
        .await,
        Some(LabelRpcResult::Accepted),
    );
    assert_eq!(
        table.get_labels(&mob_scope).await,
        BTreeMap::from([("scope".to_string(), "mob".to_string())]),
    );
    assert_eq!(
        table.get_labels(&run_scope).await,
        BTreeMap::from([("scope".to_string(), "run".to_string())]),
    );

    assert_eq!(
        dispatch_label_method(&table, mob_id, "meta/mob_labels/get", &json!({})).await,
        Some(LabelRpcResult::Labels(BTreeMap::from([(
            "scope".to_string(),
            "mob".to_string(),
        )]))),
    );
    assert_eq!(
        dispatch_label_method(
            &table,
            mob_id,
            "meta/run_labels/get",
            &json!({ "run_id": RUN_ID }),
        )
        .await,
        Some(LabelRpcResult::Labels(BTreeMap::from([(
            "scope".to_string(),
            "run".to_string(),
        )]))),
    );

    assert_eq!(
        dispatch_label_method(&table, mob_id, "meta/mob_labels/delete", &json!({}),).await,
        Some(LabelRpcResult::Accepted),
    );
    assert!(table.get_labels(&mob_scope).await.is_empty());
    assert_eq!(
        table.get_labels(&run_scope).await,
        BTreeMap::from([("scope".to_string(), "run".to_string())]),
        "mob delete must not mutate the run scope",
    );

    assert_eq!(
        dispatch_label_method(
            &table,
            mob_id,
            "meta/run_labels/delete",
            &json!({ "run_id": RUN_ID }),
        )
        .await,
        Some(LabelRpcResult::Accepted),
    );
    assert!(table.get_labels(&run_scope).await.is_empty());

    assert_eq!(
        dispatch_label_method(&table, mob_id, "meta/run_labels/get", &json!({})).await,
        Some(LabelRpcResult::InvalidParams("run_id required".to_string(),)),
    );
    for params in [
        json!({ "run_id": null }),
        json!({ "run_id": 7 }),
        json!({ "run_id": "" }),
    ] {
        assert_eq!(
            dispatch_label_method(&table, mob_id, "meta/run_labels/get", &params).await,
            Some(LabelRpcResult::InvalidParams("run_id required".to_string())),
        );
    }
    for params in [
        json!({ "labels": "not-an-object" }),
        json!({ "labels": { "invalid": 7 } }),
    ] {
        assert!(matches!(
            dispatch_label_method(&table, mob_id, "meta/mob_labels/set", &params).await,
            Some(LabelRpcResult::InvalidParams(_))
        ));
    }
    assert_eq!(
        dispatch_label_method(&table, mob_id, "meta/not_labels/get", &json!({})).await,
        None,
        "the domain handler must not absorb unrelated RPC methods",
    );
}

/// One `RuntimeMetadataTable`, two transports. A write over stdio is visible
/// over HTTP and vice versa. The merge must not introduce a second authority
/// (nor re-derive the mob id differently: stdio uses `runtime.mob_id()`, the
/// console uses `runtime.handle().mob_id()`).
#[tokio::test]
async fn both_transports_share_one_metadata_table_and_one_mob_scope() {
    let fixture = label_fixture().await;
    let console = fixture.console(false);

    let written = stdio(
        &fixture.runtime,
        "meta/mob_labels/set",
        json!({ "labels": { "written_by": "stdio" } }),
    )
    .await;
    assert_no_error("stdio mob_labels/set", &written);

    let read_back = http(&console, "meta/mob_labels/get", json!({})).await;
    assert_success_envelope(
        "http mob_labels/get reads the stdio write",
        &read_back,
        &json!({ "labels": { "written_by": "stdio" } }),
    );

    let overwritten = http(
        &console,
        "meta/run_labels/set",
        json!({ "run_id": RUN_ID, "labels": { "written_by": "http" } }),
    )
    .await;
    assert_no_error("http run_labels/set", &overwritten);

    let stdio_read = stdio(
        &fixture.runtime,
        "meta/run_labels/get",
        json!({ "run_id": RUN_ID }),
    )
    .await;
    assert_success_envelope(
        "stdio run_labels/get reads the http write",
        &stdio_read,
        &json!({ "labels": { "written_by": "http" } }),
    );

    fixture.shutdown().await;
}

/// Scope and clearing semantics that a unified `dispatch_label_method` must
/// preserve:
/// - `set` with an absent `labels` key, and with `{}`, are both accepted and
///   both *clear* the scope (`parse_labels_param(None) -> Ok(empty)` and
///   `set_labels` removes on empty).
/// - `delete` on a never-set scope is accepted, not "not found" (the
///   dispatcher discards the returned `Option`).
/// - `mob_labels/get` is mob-scoped even when the caller passes `run_id`;
///   the scope is chosen by method name, never by params.
#[tokio::test]
async fn label_scope_and_clearing_semantics_are_pinned_on_both_transports() {
    let fixture = label_fixture().await;
    let console = fixture.console(false);

    // delete on a never-set run scope -> accepted.
    let never_set = stdio(
        &fixture.runtime,
        "meta/run_labels/delete",
        json!({ "run_id": "run-never-set" }),
    )
    .await;
    assert_success_envelope(
        "stdio run_labels/delete on unset scope",
        &never_set,
        &json!({ "accepted": true }),
    );
    let never_set_http = http(
        &console,
        "meta/run_labels/delete",
        json!({ "run_id": "run-never-set-http" }),
    )
    .await;
    assert_success_envelope(
        "http run_labels/delete on unset scope",
        &never_set_http,
        &json!({ "accepted": true }),
    );

    // set with `{}` clears.
    assert_no_error(
        "stdio mob_labels/set seed",
        &stdio(
            &fixture.runtime,
            "meta/mob_labels/set",
            json!({ "labels": { "a": "1" } }),
        )
        .await,
    );
    let cleared = stdio(
        &fixture.runtime,
        "meta/mob_labels/set",
        json!({ "labels": {} }),
    )
    .await;
    assert_success_envelope(
        "stdio mob_labels/set with empty map",
        &cleared,
        &json!({ "accepted": true }),
    );
    assert_success_envelope(
        "stdio mob_labels/get after empty set",
        &stdio(&fixture.runtime, "meta/mob_labels/get", json!({})).await,
        &json!({ "labels": {} }),
    );

    // ... and so does `set` with the `labels` key absent entirely - the
    // surprising case (`parse_labels_param(None)` is `Ok(empty)`, not an
    // error). Mirrored on both transports so a merge cannot keep one arm's
    // behaviour and lose the other's.
    assert_no_error(
        "stdio mob_labels/set seed for absent-labels",
        &stdio(
            &fixture.runtime,
            "meta/mob_labels/set",
            json!({ "labels": { "a": "1" } }),
        )
        .await,
    );
    let stdio_absent = stdio(&fixture.runtime, "meta/mob_labels/set", json!({})).await;
    assert_success_envelope(
        "stdio mob_labels/set with absent labels key",
        &stdio_absent,
        &json!({ "accepted": true }),
    );
    assert_success_envelope(
        "stdio mob_labels/get after absent-labels set",
        &stdio(&fixture.runtime, "meta/mob_labels/get", json!({})).await,
        &json!({ "labels": {} }),
    );

    assert_no_error(
        "http mob_labels/set seed",
        &http(
            &console,
            "meta/mob_labels/set",
            json!({ "labels": { "b": "2" } }),
        )
        .await,
    );
    let http_empty = http(&console, "meta/mob_labels/set", json!({ "labels": {} })).await;
    assert_success_envelope(
        "http mob_labels/set with empty map",
        &http_empty,
        &json!({ "accepted": true }),
    );
    assert_success_envelope(
        "http mob_labels/get after empty set",
        &http(&console, "meta/mob_labels/get", json!({})).await,
        &json!({ "labels": {} }),
    );

    assert_no_error(
        "http mob_labels/set seed for absent-labels",
        &http(
            &console,
            "meta/mob_labels/set",
            json!({ "labels": { "b": "2" } }),
        )
        .await,
    );
    let http_absent = http(&console, "meta/mob_labels/set", json!({})).await;
    assert_success_envelope(
        "http mob_labels/set with absent labels key",
        &http_absent,
        &json!({ "accepted": true }),
    );
    assert_success_envelope(
        "http mob_labels/get after absent-labels set",
        &http(&console, "meta/mob_labels/get", json!({})).await,
        &json!({ "labels": {} }),
    );

    // mob_labels/get ignores run_id: the scope comes from the method name.
    assert_no_error(
        "seed mob scope",
        &stdio(
            &fixture.runtime,
            "meta/mob_labels/set",
            json!({ "labels": { "scope": "mob" } }),
        )
        .await,
    );
    assert_no_error(
        "seed run scope",
        &stdio(
            &fixture.runtime,
            "meta/run_labels/set",
            json!({ "run_id": RUN_ID, "labels": { "scope": "run" } }),
        )
        .await,
    );
    assert_success_envelope(
        "stdio mob_labels/get ignores run_id",
        &stdio(
            &fixture.runtime,
            "meta/mob_labels/get",
            json!({ "run_id": RUN_ID }),
        )
        .await,
        &json!({ "labels": { "scope": "mob" } }),
    );
    assert_success_envelope(
        "http mob_labels/get ignores run_id",
        &http(&console, "meta/mob_labels/get", json!({ "run_id": RUN_ID })).await,
        &json!({ "labels": { "scope": "mob" } }),
    );

    fixture.shutdown().await;
}

/// Invalid params on the run-scoped verbs. Same code (-32602), same absence
/// of a `data` key - but the *messages differ by transport*, because stdio's
/// `label_response` prefixes `"Invalid params: "` and the console's
/// `invalid_params` does not. The shared domain deliberately leaves this
/// projection policy to each transport, so this pins that there are two.
#[tokio::test]
async fn invalid_params_envelopes_pin_the_transport_message_prefix_divergence() {
    let fixture = label_fixture().await;
    let console = fixture.console(false);

    // Missing run_id: fixed, fully pinnable message on both sides.
    for method in [
        "meta/run_labels/set",
        "meta/run_labels/get",
        "meta/run_labels/delete",
    ] {
        let params = if method.ends_with("/set") {
            json!({ "labels": { "env": "dev" } })
        } else {
            json!({})
        };
        assert_error_envelope(
            &format!("stdio {method} without run_id"),
            &stdio(&fixture.runtime, method, params.clone()).await,
            -32602,
            "Invalid params: run_id required",
            None,
        );
        assert_error_envelope(
            &format!("http {method} without run_id"),
            &http(&console, method, params).await,
            -32602,
            "run_id required",
            None,
        );
    }

    // Empty-string run_id is treated as absent, not as a scope named "".
    assert_error_envelope(
        "stdio run_labels/get with empty run_id",
        &stdio(
            &fixture.runtime,
            "meta/run_labels/get",
            json!({ "run_id": "" }),
        )
        .await,
        -32602,
        "Invalid params: run_id required",
        None,
    );
    assert_error_envelope(
        "http run_labels/get with empty run_id",
        &http(&console, "meta/run_labels/get", json!({ "run_id": "" })).await,
        -32602,
        "run_id required",
        None,
    );

    // Non string->string labels. The tail of the message is a serde_json
    // error string (version dependent), so pin the stable head only.
    for (method, params) in [
        ("meta/mob_labels/set", json!({ "labels": { "a": 1 } })),
        (
            "meta/run_labels/set",
            json!({ "run_id": RUN_ID, "labels": { "a": 1 } }),
        ),
    ] {
        let stdio_envelope = stdio(&fixture.runtime, method, params.clone()).await;
        assert_eq!(
            key_set(&stdio_envelope),
            vec!["error".to_string(), "id".to_string(), "jsonrpc".to_string()],
            "stdio {method} bad labels: {stdio_envelope:#?}"
        );
        assert_eq!(
            key_set(&stdio_envelope["error"]),
            vec!["code".to_string(), "message".to_string()],
            "stdio {method} bad labels must carry no data key: {stdio_envelope:#?}"
        );
        assert_eq!(stdio_envelope["error"]["code"], json!(-32602));
        let stdio_message = stdio_envelope["error"]["message"]
            .as_str()
            .expect("stdio message string");
        assert!(
            stdio_message.starts_with("Invalid params: labels must be a map of string to string"),
            "stdio {method} bad labels message: {stdio_message}"
        );

        let http_envelope = http(&console, method, params).await;
        assert_eq!(
            key_set(&http_envelope["error"]),
            vec!["code".to_string(), "message".to_string()],
            "http {method} bad labels must carry no data key: {http_envelope:#?}"
        );
        assert_eq!(http_envelope["error"]["code"], json!(-32602));
        let http_message = http_envelope["error"]["message"]
            .as_str()
            .expect("http message string");
        assert!(
            http_message.starts_with("labels must be a map of string to string"),
            "http {method} bad labels message: {http_message}"
        );
        assert!(
            !http_message.starts_with("Invalid params:"),
            "the console must NOT prefix `Invalid params:` today: {http_message}"
        );
    }

    fixture.shutdown().await;
}

/// Raw-bytes pins. Re-parsing normalises key order, so these read the wire
/// text directly. stdio serializes the `JsonRpcResponse` struct (declaration
/// order); the console converts to `Value` first and serde_json is built
/// without `preserve_order`, so its `Map` is a `BTreeMap` and keys come out
/// lexicographic. Same envelope, different bytes.
#[tokio::test]
async fn wire_serialization_diverges_between_transports() {
    let fixture = label_fixture().await;
    let console = fixture.console(false);

    let stdio_success = stdio_raw(&fixture.runtime, "meta/mob_labels/get", json!({})).await;
    assert!(
        stdio_success.starts_with(r#"{"jsonrpc":"2.0","id":"golden","result":"#),
        "stdio success wire order is jsonrpc,id,result: {stdio_success}"
    );

    let http_success = http_raw(&console, "meta/mob_labels/get", json!({})).await;
    assert!(
        http_success.starts_with(r#"{"id":"golden","jsonrpc":"2.0","result":"#),
        "console success wire order is lexicographic (id,jsonrpc,result): {http_success}"
    );

    let stdio_error = stdio_raw(&fixture.runtime, "meta/run_labels/get", json!({})).await;
    assert!(
        stdio_error.starts_with(r#"{"jsonrpc":"2.0","id":"golden","error":"#),
        "stdio error wire order is jsonrpc,id,error: {stdio_error}"
    );

    let http_error = http_raw(&console, "meta/run_labels/get", json!({})).await;
    assert!(
        http_error.starts_with(r#"{"error":"#),
        "console error wire order is lexicographic (error first): {http_error}"
    );

    // Label maps serialize lexicographically on both transports.
    assert_no_error(
        "seed for label ordering",
        &stdio(
            &fixture.runtime,
            "meta/mob_labels/set",
            json!({ "labels": { "zebra": "z", "alpha": "a", "middle": "m" } }),
        )
        .await,
    );
    let ordered_stdio = stdio_raw(&fixture.runtime, "meta/mob_labels/get", json!({})).await;
    assert!(
        ordered_stdio.contains(r#""labels":{"alpha":"a","middle":"m","zebra":"z"}"#),
        "stdio label keys must be lexicographic: {ordered_stdio}"
    );
    let ordered_http = http_raw(&console, "meta/mob_labels/get", json!({})).await;
    assert!(
        ordered_http.contains(r#""labels":{"alpha":"a","middle":"m","zebra":"z"}"#),
        "console label keys must be lexicographic: {ordered_http}"
    );

    // Mutating notifications execute the shared domain operation before each
    // transport applies its distinct response-suppression policy.
    let stdio_notification_request = json!({
        "jsonrpc": "2.0",
        "method": "meta/mob_labels/set",
        "params": { "labels": { "notification": "stdio" } },
    })
    .to_string();
    let stdio_notification = handle_unified_rpc_json(
        &fixture.runtime,
        &stdio_notification_request,
        Duration::from_secs(5),
        None,
        None,
    )
    .await;
    assert_eq!(
        stdio_notification, "",
        "stdio must answer a notification with an empty string"
    );
    assert_eq!(
        http(&console, "meta/mob_labels/get", json!({})).await["result"]["labels"],
        json!({ "notification": "stdio" }),
        "stdio notification must mutate before its response is suppressed"
    );

    let http_notification_request = json!({
        "jsonrpc": "2.0",
        "method": "meta/mob_labels/set",
        "params": { "labels": { "notification": "http" } },
    })
    .to_string();
    let response = console
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/console/rpc")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(http_notification_request))
                .expect("notification request"),
        )
        .await
        .expect("notification response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("notification body");
    let envelope: Value = serde_json::from_slice(&body).expect("notification json");
    assert_eq!(
        key_set(&envelope),
        vec![
            "id".to_string(),
            "jsonrpc".to_string(),
            "result".to_string()
        ],
        "console answers notifications with a full envelope: {envelope:#?}"
    );
    assert_eq!(
        envelope["id"],
        Value::Null,
        "console substitutes a null id for a notification: {envelope:#?}"
    );
    assert_eq!(
        stdio(&fixture.runtime, "meta/mob_labels/get", json!({})).await["result"]["labels"],
        json!({ "notification": "http" }),
        "console notification must mutate before its null-id envelope is projected"
    );

    fixture.shutdown().await;
}

/// The console can be constructed without a metadata table; the stdio
/// runtime always owns one (`UnifiedRuntime::metadata_table` returns a
/// borrow, not an `Option`). That asymmetry means a merged dispatcher has to
/// decide what an absent table means. Today the console answers -32602 with
/// a bare message, *and* advertises none of the six - the whole label block
/// in the capability list sits behind `if metadata_table.is_some()`.
///
/// Both halves matter to item 9: the dispatch answer and the advertisement
/// are computed from the same `Option` in two different places, so a merge
/// that centralises one without the other makes the console UI offer a verb
/// the dispatcher refuses (or hide one it would serve).
#[tokio::test]
async fn console_without_a_metadata_table_refuses_and_hides_every_label_verb() {
    let fixture = label_fixture().await;
    // `console_json_router_with_runtime` passes `metadata_table: None`
    // (and `access: None`, so nothing here rides the ABAC gate) while
    // `read_only: false` keeps `can_mutate` true - the four mutators reach
    // the dispatcher rather than stopping at the read-only gate.
    let console = console_json_router_with_runtime(
        decision_state(false),
        fixture.runtime.mob_runtime().clone(),
        None,
        None,
    );

    for method in ALL_LABEL_VERBS {
        assert_error_envelope(
            &format!("http {method} without a metadata table"),
            &http(&console, method, params_for(method)).await,
            -32602,
            "metadata table not configured for this runtime",
            None,
        );
    }

    // The advertisement half. `can_mutate` is true here, so a list that
    // still carried the four mutators could only come from the
    // `metadata_table.is_some()` guard being dropped.
    let methods = advertised_methods(&http(&console, "meta/capabilities", json!({})).await);
    for method in ALL_LABEL_VERBS {
        assert!(
            !methods.iter().any(|listed| listed.as_str() == *method),
            "console without a metadata table must not advertise {method}: {methods:?}"
        );
    }

    fixture.shutdown().await;
}

// ===========================================================================
// 2. Access decision goldens - the two fail-open tables
// ===========================================================================

/// `is_console_mutating_rpc_method` is a fail-open allowlist: an unlisted
/// method counts as non-mutating and is permitted on a read-only console.
/// This pins both halves of the current classification, so dropping a mutator
/// from the table (silent privilege escalation) or adding a getter to it
/// (silent breakage) both fail here.
#[tokio::test]
async fn console_read_only_gates_exactly_the_four_label_mutators() {
    let fixture = label_fixture().await;
    let read_only = fixture.console(true);
    let writable = fixture.console(false);

    for method in MUTATING_LABEL_VERBS {
        assert_console_read_only(
            &format!("read-only console {method}"),
            &http(&read_only, method, params_for(method)).await,
        );
        // Control: the same call on a writable console dispatches.
        assert_no_error(
            &format!("writable console {method}"),
            &http(&writable, method, params_for(method)).await,
        );
    }

    // The mutator loop above ended on the two `delete` verbs, so re-seed
    // both scopes through the writable console before reading them back.
    for method in ["meta/mob_labels/set", "meta/run_labels/set"] {
        assert_no_error(
            &format!("re-seed {method}"),
            &http(&writable, method, params_for(method)).await,
        );
    }

    for method in READ_LABEL_VERBS {
        let envelope = http(&read_only, method, params_for(method)).await;
        assert_no_error(
            &format!("read-only console must still serve {method}"),
            &envelope,
        );
        assert_eq!(
            &envelope["result"],
            &expected_result_for(method, json!({ "env": "dev" })),
            "read-only console {method}: {envelope:#?}"
        );
    }

    fixture.shutdown().await;
}

/// `console_rpc_access_requirements` is a fail-open map: an unmapped method
/// yields no grant requirement and sails through the ABAC gate. All six
/// label verbs currently map to one resource-less `runtime.admin` check, and
/// this pins the denial for every one of them - including the gate ordering
/// (read-only first, then ABAC, then param validation).
#[tokio::test]
async fn console_abac_requires_unscoped_runtime_admin_for_all_six_label_verbs() {
    let mut fixture = label_fixture().await;
    let controller = AccessController::new(deny_runtime_admin_config()).expect("controller");
    fixture.runtime.set_access_controller(controller);
    // Router must be built after the controller is installed.
    let console = fixture.console(false);
    let read_only_console = fixture.console(true);

    for method in ALL_LABEL_VERBS {
        assert_label_access_denied(
            &format!("abac-denied {method}"),
            &http(&console, method, params_for(method)).await,
        );
    }

    // Passing an `identity` param must not re-target the check: the label
    // arm hands `None` to the requirement even though the function can
    // derive a target from `identity`/`member_id`/`agent_id`.
    assert_label_access_denied(
        "abac-denied mob_labels/set with an identity param",
        &http(
            &console,
            "meta/mob_labels/set",
            json!({ "labels": { "env": "dev" }, "identity": "lead" }),
        )
        .await,
    );

    // Ordering: the read-only gate runs before the ABAC gate, so a mutator
    // on a read-only console reports -32010, never -32030.
    assert_console_read_only(
        "read-only wins over abac for mob_labels/set",
        &http(
            &read_only_console,
            "meta/mob_labels/set",
            params_for("meta/mob_labels/set"),
        )
        .await,
    );

    // Ordering: the ABAC gate runs before params are validated, so an
    // unauthorized caller learns nothing about `run_id` validity.
    assert_label_access_denied(
        "abac denial precedes run_id validation",
        &http(&console, "meta/run_labels/get", json!({})).await,
    );

    fixture.shutdown().await;
}

/// The positive half: a config whose only rule grants the anonymous
/// principal an unscoped `runtime.admin` lets all six verbs through. This is
/// what pins the *identity* of the required action - a denial test alone
/// would still pass if the requirement were changed to some other action.
#[tokio::test]
async fn console_abac_grant_of_runtime_admin_alone_permits_all_six_label_verbs() {
    // Tie the pinned wire string to the constant the requirement table
    // actually names. Without this, changing `ACTION_RUNTIME_ADMIN`'s value
    // would only surface three assertions downstream (as "the grant config
    // no longer grants anything"); here it fails at the pin itself.
    assert_eq!(
        LABEL_GRANT_ACTION,
        meerkat_meta::access::ACTION_RUNTIME_ADMIN,
        "the label verbs' grant action string is the wire contract"
    );

    let mut fixture = label_fixture().await;
    let controller = AccessController::new(grant_runtime_admin_config()).expect("controller");
    fixture.runtime.set_access_controller(controller);
    let console = fixture.console(false);

    for method in ALL_LABEL_VERBS {
        assert_no_error(
            &format!("runtime.admin grant permits {method}"),
            &http(&console, method, params_for(method)).await,
        );
    }

    fixture.shutdown().await;
}

/// The asymmetry item 9 has to resolve: the stdio dispatcher consults
/// neither `RuntimeDecisionState` (it is not even a parameter of
/// `handle_unified_rpc_json`) nor the `AccessController`. With a runtime
/// whose controller denies `runtime.admin`, every label verb still succeeds
/// over stdio while the same call over HTTP is refused.
///
/// A merge that reuses one dispatch body either grows gates on stdio (a
/// behaviour change for every stdio host) or loses them on HTTP (a security
/// regression). Neither is acceptable by accident.
#[tokio::test]
async fn stdio_label_verbs_consult_neither_console_read_only_nor_abac() {
    let mut fixture = label_fixture().await;
    let controller = AccessController::new(deny_runtime_admin_config()).expect("controller");
    fixture.runtime.set_access_controller(controller);
    let read_only_console = fixture.console(true);

    for method in ALL_LABEL_VERBS {
        let envelope = stdio(&fixture.runtime, method, params_for(method)).await;
        assert_no_error(
            &format!("stdio {method} is ungated by console policy"),
            &envelope,
        );
        assert_eq!(
            key_set(&envelope),
            vec![
                "id".to_string(),
                "jsonrpc".to_string(),
                "result".to_string()
            ],
            "stdio {method}: {envelope:#?}"
        );
    }

    // Same runtime, same instant, over HTTP: refused. Mutators trip the
    // read-only gate first; the readers reach the ABAC gate.
    for method in MUTATING_LABEL_VERBS {
        assert_console_read_only(
            &format!("http {method} while stdio permits it"),
            &http(&read_only_console, method, params_for(method)).await,
        );
    }
    for method in READ_LABEL_VERBS {
        assert_label_access_denied(
            &format!("http {method} while stdio permits it"),
            &http(&read_only_console, method, params_for(method)).await,
        );
    }

    fixture.shutdown().await;
}

// ===========================================================================
// 3. Capability advertisement parity
// ===========================================================================

fn advertised_methods(envelope: &Value) -> Vec<String> {
    envelope["result"]["methods"]
        .as_array()
        .unwrap_or_else(|| panic!("capabilities must carry a methods array: {envelope:#?}"))
        .iter()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

/// `meta/capabilities` advertises the label verbs on three different
/// schedules, and the two dispatchers do not agree. stdio lists all six
/// unconditionally. The console lists the two getters only when a metadata
/// table is wired, the four mutators only when the caller may mutate, and
/// drops all six when an enforced access view withholds `runtime.admin`.
/// Unifying the dispatch arms without unifying this list changes what the
/// console UI believes it can call.
#[tokio::test]
async fn capabilities_advertise_label_verbs_differently_per_transport() {
    let fixture = label_fixture().await;

    let stdio_methods =
        advertised_methods(&stdio(&fixture.runtime, "meta/capabilities", json!({})).await);
    for method in ALL_LABEL_VERBS {
        assert!(
            stdio_methods
                .iter()
                .any(|listed| listed.as_str() == *method),
            "stdio capabilities must advertise {method} unconditionally: {stdio_methods:?}"
        );
    }

    let writable_methods =
        advertised_methods(&http(&fixture.console(false), "meta/capabilities", json!({})).await);
    for method in ALL_LABEL_VERBS {
        assert!(
            writable_methods
                .iter()
                .any(|listed| listed.as_str() == *method),
            "writable console must advertise {method}: {writable_methods:?}"
        );
    }

    let read_only_methods =
        advertised_methods(&http(&fixture.console(true), "meta/capabilities", json!({})).await);
    for method in READ_LABEL_VERBS {
        assert!(
            read_only_methods
                .iter()
                .any(|listed| listed.as_str() == *method),
            "read-only console must still advertise {method}: {read_only_methods:?}"
        );
    }
    for method in MUTATING_LABEL_VERBS {
        assert!(
            !read_only_methods
                .iter()
                .any(|listed| listed.as_str() == *method),
            "read-only console must not advertise {method}: {read_only_methods:?}"
        );
    }

    fixture.shutdown().await;
}

/// The access-view half of the capability projection: with ABAC enforced and
/// `runtime.admin` withheld, the console strips every label verb from the
/// advertised list (they are all resource-less, so the grant-intersection
/// probe removes them). stdio, which has no view, keeps advertising all six.
#[tokio::test]
async fn console_capabilities_drop_label_verbs_without_the_runtime_admin_grant() {
    let mut fixture = label_fixture().await;
    let controller = AccessController::new(deny_runtime_admin_config()).expect("controller");
    fixture.runtime.set_access_controller(controller);
    let console = fixture.console(false);

    let console_methods = advertised_methods(&http(&console, "meta/capabilities", json!({})).await);
    for method in ALL_LABEL_VERBS {
        assert!(
            !console_methods
                .iter()
                .any(|listed| listed.as_str() == *method),
            "console must not advertise {method} without the {LABEL_GRANT_ACTION} grant: \
             {console_methods:?}"
        );
    }

    let stdio_methods =
        advertised_methods(&stdio(&fixture.runtime, "meta/capabilities", json!({})).await);
    for method in ALL_LABEL_VERBS {
        assert!(
            stdio_methods
                .iter()
                .any(|listed| listed.as_str() == *method),
            "stdio capabilities ignore the access view and still advertise {method}: \
             {stdio_methods:?}"
        );
    }

    fixture.shutdown().await;
}
