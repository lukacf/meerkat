#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use meerkat_meta::{
    AuthPolicy, AuthProvider, BigQueryNaming, BigQuerySessionStoreAdapter, ConsoleAccessRequest,
    ConsolePolicy, ConsoleRestJsonRequest, DiscoverySpec, EventEnvelope, JsonFileSessionStore,
    JwtValidationConfig, MetaConfig, ModuleConfig, ModuleRouteRequest, PreSpawnData, RestartPolicy,
    RuntimeDecisionInputs, RuntimeOpsPolicy, SessionPersistenceRow, SessionStoreContract,
    SessionStoreKind, TrustedOidcRuntimeConfig, UnifiedEvent, build_runtime_decision_state,
    handle_console_rest_json_route, handle_meta_rpc_json, route_module_call,
    session_store_contracts, start_meta_runtime, validate_jwt_locally,
};
use serde_json::{Value, json};
use sha2::Sha256;
use tempfile::tempdir;

#[path = "support/bigquery_http_mock.rs"]
mod bigquery_http_mock;

use bigquery_http_mock::{MockHttpResponse, MockHttpServer};

type HmacSha256 = Hmac<Sha256>;

const BOUNDARY_ENV_KEY: &str = "META_MODULE_BOUNDARY";
const BOUNDARY_ENV_VALUE_MCP: &str = "mcp";

fn fixture_binary_path() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_mcp_fixture") {
        return PathBuf::from(path);
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let binary_path = workspace_root
        .join("target")
        .join("debug")
        .join("mcp_fixture");
    if binary_path.exists() {
        return binary_path;
    }

    let status = Command::new("cargo")
        .args(["build", "-p", "meerkat-meta", "--bin", "mcp_fixture"])
        .current_dir(workspace_root)
        .status()
        .expect("build mcp_fixture");
    assert!(status.success(), "building mcp_fixture must succeed");
    binary_path
}

fn fixture_module(id: &str, fixture_binary: &std::path::Path) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: fixture_binary.display().to_string(),
        args: vec!["--module".to_string(), id.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn mcp_env(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut env = vec![(
        BOUNDARY_ENV_KEY.to_string(),
        BOUNDARY_ENV_VALUE_MCP.to_string(),
    )];
    env.extend(
        extra
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string())),
    );
    env
}

fn shell_module(id: &str, script: &str) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn parse_response(line: &str) -> Value {
    serde_json::from_str(line).expect("valid rpc response json")
}

fn store_record_count(response: &Value, store: &str) -> Option<u64> {
    response["result"]["stores"]
        .as_array()
        .and_then(|stores| {
            stores
                .iter()
                .find(|entry| entry.get("store").and_then(Value::as_str) == Some(store))
        })
        .and_then(|entry| entry.get("record_count"))
        .and_then(Value::as_u64)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SdkMappedOutcome {
    Success {
        id: Value,
        result: Value,
    },
    Error {
        id: Value,
        code: i64,
        message: String,
    },
}

fn map_ts_sdk_response(response: &Value) -> SdkMappedOutcome {
    let jsonrpc = response
        .get("jsonrpc")
        .and_then(Value::as_str)
        .expect("TS SDK expects jsonrpc string");
    assert_eq!(jsonrpc, "2.0", "TS SDK requires JSON-RPC 2.0");

    let id = response
        .get("id")
        .cloned()
        .expect("TS SDK expects response id");

    if let Some(error) = response.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .expect("TS SDK expects error.code");
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .expect("TS SDK expects error.message");
        SdkMappedOutcome::Error { id, code, message }
    } else {
        let result = response
            .get("result")
            .cloned()
            .expect("TS SDK expects result for successful responses");
        SdkMappedOutcome::Success { id, result }
    }
}

fn map_python_sdk_response(response: &Value) -> SdkMappedOutcome {
    let payload = response
        .as_object()
        .expect("Python SDK expects response dict payload");
    let jsonrpc = payload
        .get("jsonrpc")
        .and_then(Value::as_str)
        .expect("Python SDK expects jsonrpc");
    assert_eq!(jsonrpc, "2.0", "Python SDK requires JSON-RPC 2.0");

    let id = payload
        .get("id")
        .cloned()
        .expect("Python SDK expects response id");

    if let Some(error) = payload.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .expect("Python SDK expects integer error code");
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .expect("Python SDK expects error message");
        SdkMappedOutcome::Error { id, code, message }
    } else {
        let result = payload
            .get("result")
            .cloned()
            .expect("Python SDK expects result payload");
        SdkMappedOutcome::Success { id, result }
    }
}

fn subscribed_events(response: &Value) -> Vec<Value> {
    response
        .get("result")
        .and_then(|result| result.get("events"))
        .and_then(Value::as_array)
        .cloned()
        .expect("meta/events/subscribe should return result.events[]")
}

fn subscribed_keep_alive(response: &Value) -> Value {
    response
        .get("result")
        .and_then(|result| result.get("keep_alive"))
        .cloned()
        .expect("meta/events/subscribe should return result.keep_alive")
}

fn subscribed_keep_alive_comment(response: &Value) -> String {
    response
        .get("result")
        .and_then(|result| result.get("keep_alive_comment"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .expect("meta/events/subscribe should return result.keep_alive_comment")
}

fn subscribed_event_frames(response: &Value) -> Vec<String> {
    response
        .get("result")
        .and_then(|result| result.get("event_frames"))
        .and_then(Value::as_array)
        .cloned()
        .expect("meta/events/subscribe should return result.event_frames[]")
        .into_iter()
        .map(|frame| {
            frame
                .as_str()
                .map(ToString::to_string)
                .expect("result.event_frames[] should be strings")
        })
        .collect()
}

fn release_json() -> String {
    include_str!("../assets/release-targets.json").to_string()
}

fn sign_hs256(payload: Value, secret: &str, kid: &str) -> String {
    let header = json!({"alg":"HS256","typ":"JWT","kid":kid});
    let header_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("encode header"));
    let payload_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("encode payload"));
    let signing_input = format!("{header_b64}.{payload_b64}");
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("hmac init");
    mac.update(signing_input.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{signing_input}.{signature}")
}

fn b64_json(value: Value) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).expect("serialize json"))
}

fn trusted_token(payload: Value, kid: &str, secret: &str) -> String {
    sign_hs256(payload, secret, kid)
}

fn trusted_oidc() -> TrustedOidcRuntimeConfig {
    TrustedOidcRuntimeConfig {
        discovery_json:
            r#"{"issuer":"https://trusted.meta.localhost","jwks_uri":"https://trusted.meta.localhost/.well-known/jwks.json"}"#
                .to_string(),
        jwks_json: r#"{"keys":[{"kid":"kid-current","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtY3VycmVudC1zZWNyZXQ"},{"kid":"kid-next","kty":"oct","alg":"HS256","k":"cGhhc2U3LXRydXN0ZWQtbmV4dC1zZWNyZXQ"}]}"#.to_string(),
        audience: "meerkat-console".to_string(),
        require_verified_email: false,
    }
}

fn trusted_toml() -> String {
    r#"
[[modules]]
id = "router"
command = "router-bin"
args = ["--mode", "fast"]
restart_policy = "always"

[[modules]]
id = "delivery"
command = "delivery-bin"
args = ["--sink", "test"]
restart_policy = "on_failure"
"#
    .to_string()
}

fn runtime_with_router_and_delivery() -> meerkat_meta::MetaRuntimeHandle {
    let config = MetaConfig {
        modules: vec![
            shell_module(
                "router",
                r#"printf '%s\n' '{"event_id":"evt-router","source":"module","timestamp_ms":10,"event":{"kind":"module","module":"router","event_type":"response","payload":{"via":"router","ok":true}}}'"#,
            ),
            shell_module(
                "delivery",
                r#"printf '%s\n' '{"event_id":"evt-delivery","source":"module","timestamp_ms":20,"event":{"kind":"module","module":"delivery","event_type":"ready","payload":{"sink":"memory"}}}'"#,
            ),
        ],
        discovery: DiscoverySpec {
            namespace: "phase3c".to_string(),
            modules: vec!["router".to_string()],
        },
        pre_spawn: vec![
            PreSpawnData {
                module_id: "router".to_string(),
                env: vec![],
            },
            PreSpawnData {
                module_id: "delivery".to_string(),
                env: vec![],
            },
        ],
    };

    start_meta_runtime(config, vec![], Duration::from_secs(1)).expect("runtime starts")
}

fn runtime_with_router_and_delivery_mcp() -> meerkat_meta::MetaRuntimeHandle {
    let fixture_binary = fixture_binary_path();
    let config = MetaConfig {
        modules: vec![
            fixture_module("router", &fixture_binary),
            fixture_module("delivery", &fixture_binary),
        ],
        discovery: DiscoverySpec {
            namespace: "phase3c".to_string(),
            modules: vec!["router".to_string()],
        },
        pre_spawn: vec![
            PreSpawnData {
                module_id: "router".to_string(),
                env: mcp_env(&[]),
            },
            PreSpawnData {
                module_id: "delivery".to_string(),
                env: mcp_env(&[]),
            },
        ],
    };

    start_meta_runtime(config, vec![], Duration::from_secs(2)).expect("runtime starts")
}

fn runtime_with_phase6_agent_events() -> meerkat_meta::MetaRuntimeHandle {
    let mut runtime = runtime_with_router_and_delivery();
    runtime.inject_test_events(vec![
        EventEnvelope {
            event_id: "evt-agent-a-0".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 5,
            event: UnifiedEvent::Agent {
                agent_id: "agent-alpha".to_string(),
                event_type: "interaction.start".to_string(),
                payload: None,
            },
        },
        EventEnvelope {
            event_id: "evt-agent-b-0".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 6,
            event: UnifiedEvent::Agent {
                agent_id: "agent-beta".to_string(),
                event_type: "interaction.start".to_string(),
                payload: None,
            },
        },
        EventEnvelope {
            event_id: "evt-agent-a-1".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 11,
            event: UnifiedEvent::Agent {
                agent_id: "agent-alpha".to_string(),
                event_type: "tick".to_string(),
                payload: None,
            },
        },
        EventEnvelope {
            event_id: "evt-agent-b-1".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 12,
            event: UnifiedEvent::Agent {
                agent_id: "agent-beta".to_string(),
                event_type: "tick".to_string(),
                payload: None,
            },
        },
        EventEnvelope {
            event_id: "evt-agent-a-2".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 13,
            event: UnifiedEvent::Agent {
                agent_id: "agent-alpha".to_string(),
                event_type: "tick".to_string(),
                payload: None,
            },
        },
        EventEnvelope {
            event_id: "evt-agent-a-3".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 14,
            event: UnifiedEvent::Agent {
                agent_id: "agent-alpha".to_string(),
                event_type: "interaction.reply".to_string(),
                payload: None,
            },
        },
    ]);
    runtime
}

fn decision_state(require_app_auth: bool) -> meerkat_meta::RuntimeDecisionState {
    build_runtime_decision_state(RuntimeDecisionInputs {
        bigquery: BigQueryNaming {
            dataset: "phase3c_dataset".to_string(),
            table: "phase3c_table".to_string(),
        },
        trusted_meta_toml: trusted_toml(),
        auth: AuthPolicy {
            default_provider: AuthProvider::GoogleOAuth,
            email_allowlist: vec![
                "alice@example.com".to_string(),
                "svc:deploy-bot".to_string(),
            ],
        },
        trusted_oidc: trusted_oidc(),
        console: ConsolePolicy {
            require_app_auth,
            ..ConsolePolicy::default()
        },
        ops: RuntimeOpsPolicy::default(),
        release_metadata_json: release_json(),
    })
    .expect("decision state builds")
}

#[test]
#[ignore]
fn choke_101_rpc_ingress_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery();
    let observed = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-101","method":"meta/status","params":{}}"#,
        Duration::from_secs(1),
    ));

    assert_eq!(
        observed,
        json!({
            "jsonrpc":"2.0",
            "id":"choke-101",
            "result":{
                "contract_version":"0.5.0",
                "running":true,
                "loaded_modules":["router"]
            }
        }),
        "CHOKE-101: JSON-RPC ingress/method-id-params semantics verified"
    );
}

#[test]
#[ignore]
fn choke_102_module_router_handoff_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery_mcp();
    let observed = route_module_call(
        &runtime,
        &ModuleRouteRequest {
            module_id: "router".to_string(),
            method: "routing.resolve".to_string(),
            params: json!({"recipient":"hello@example.com"}),
        },
        Duration::from_secs(1),
    )
    .expect("module route call should succeed on wired boundary");

    runtime.shutdown();

    assert_eq!(
        observed.payload,
        json!({"sink":"email","target_module":"delivery"}),
        "CHOKE-102: router->module typed error/result contract verified"
    );
}

#[tokio::test]
#[ignore]
async fn choke_103_session_store_handoff_target_defined_red() {
    let observed = decision_state(true);
    let contracts = session_store_contracts(&observed);
    let temp = tempdir().expect("tempdir");
    let store_path = temp.path().join("sessions.json");
    let json_store = JsonFileSessionStore::new(&store_path);
    let bq_adapter = BigQuerySessionStoreAdapter::new_native(
        observed.bigquery.dataset.clone(),
        observed.bigquery.table.clone(),
    );

    assert_eq!(
        (
            contracts,
            json_store.lock_path().exists(),
            bq_adapter.table_ref()
        ),
        (
            vec![
                SessionStoreContract {
                    store: SessionStoreKind::BigQuery,
                    latest_row_per_session: true,
                    tombstones_supported: true,
                    dedup_read_path: true,
                    file_locking: false,
                    crash_recovery: false,
                    bigquery_dataset: Some("phase3c_dataset".to_string()),
                    bigquery_table: Some("phase3c_table".to_string()),
                },
                SessionStoreContract {
                    store: SessionStoreKind::JsonFile,
                    latest_row_per_session: true,
                    tombstones_supported: true,
                    dedup_read_path: true,
                    file_locking: true,
                    crash_recovery: true,
                    bigquery_dataset: None,
                    bigquery_table: None,
                },
            ],
            false,
            "phase3c_dataset.phase3c_table".to_string()
        ),
        "CHOKE-103: session-store handoff preserves concrete backend wiring (JSON lock path and BigQuery table path) with latest-row+tombstone contracts"
    );
}

#[test]
#[ignore]
fn choke_104_event_bus_to_sse_bridge_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery();
    let observed = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-104","method":"meta/events/subscribe","params":{"scope":"mob"}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        observed,
        json!({
            "jsonrpc":"2.0",
            "id":"choke-104",
            "result":{
                "scope":"mob",
                "replay_from_event_id":null,
                "keep_alive":{
                    "interval_ms":15000,
                    "event":"keep-alive"
                },
                "keep_alive_comment":": keep-alive\n\n",
                "event_frames":[
                    "id: evt-router\nevent: response\ndata: {\"kind\":\"module\",\"module\":\"router\",\"event_type\":\"response\",\"payload\":{\"ok\":true,\"via\":\"router\"}}\n\n"
                ],
                "events":[
                    {
                        "event_id":"evt-router",
                        "source":"module",
                        "timestamp_ms":10,
                        "event":{
                            "kind":"module",
                            "module":"router",
                            "event_type":"response",
                            "payload":{"via":"router","ok":true}
                        }
                    }
                ]
            }
        }),
        "CHOKE-104: event bus->SSE bridge exposes deterministic event envelope for SSE id/event/data mapping"
    );

    let events = subscribed_events(&observed);
    let event = &events[0];
    let sse_id = event
        .get("event_id")
        .and_then(Value::as_str)
        .expect("event_id should be present for SSE id");
    let sse_event = event
        .get("event")
        .and_then(|value| value.get("event_type"))
        .and_then(Value::as_str)
        .expect("event_type should be present for SSE event");
    let sse_data =
        serde_json::to_string(event.get("event").expect("event body should be present")).unwrap();
    let sse_frame = format!("id: {sse_id}\nevent: {sse_event}\ndata: {sse_data}\n\n");
    let transport_frames = subscribed_event_frames(&observed);
    assert!(
        sse_frame.starts_with("id: evt-router\nevent: response\ndata: "),
        "CHOKE-104: bridge result can be rendered as an SSE frame with id/event/data"
    );
    assert_eq!(
        subscribed_keep_alive_comment(&observed),
        ": keep-alive\n\n",
        "CHOKE-104: runtime emits concrete keep-alive SSE comment frame"
    );
    assert!(
        transport_frames.len() == 1
            && transport_frames[0].starts_with("id: evt-router\nevent: response\ndata: {")
            && transport_frames[0].ends_with("\n\n"),
        "CHOKE-104: subscribe response includes concrete SSE transport frame output"
    );
}

#[test]
#[ignore]
fn choke_105_auth_to_console_route_target_defined_red() {
    let state = decision_state(true);
    let allowed = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: Some(ConsoleAccessRequest {
                provider: AuthProvider::GoogleOAuth,
                email: "alice@example.com".to_string(),
            }),
        },
    );
    let denied_provider = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: Some(ConsoleAccessRequest {
                provider: AuthProvider::GitHubOAuth,
                email: "alice@example.com".to_string(),
            }),
        },
    );
    let denied_missing = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: None,
        },
    );
    let service_allowed = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: Some(ConsoleAccessRequest {
                provider: AuthProvider::ServiceIdentity,
                email: "svc:deploy-bot".to_string(),
            }),
        },
    );

    assert_eq!(
        (
            allowed.status,
            allowed.body,
            denied_provider.status,
            denied_provider.body,
            denied_missing.status,
            denied_missing.body,
            service_allowed.status
        ),
        (
            200,
            json!({"contract_version":"0.5.0","modules":["router","delivery"]}),
            401,
            json!({"error":"unauthorized","reason":"provider_mismatch"}),
            401,
            json!({"error":"unauthorized","reason":"missing_credentials"}),
            200
        ),
        "CHOKE-105: auth middleware enforces user provider+allowlist and service-identity path with concrete error reasons"
    );
}

#[test]
#[ignore]
fn choke_107_routing_to_delivery_handoff_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery_mcp();
    let _spawn_delivery = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-107-spawn","method":"meta/spawn_member","params":{"module_id":"delivery"}}"#,
        Duration::from_secs(1),
    ));
    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-107-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","channel":"transactional"}}"#,
        Duration::from_secs(1),
    ));
    let send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"choke-107-send",
            "method":"meta/delivery/send",
            "params":{
                "resolution": resolved["result"].clone(),
                "payload": {"message":"hi"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        (
            resolved["result"]["target_module"].clone(),
            resolved["result"]["sink"].clone(),
            send["result"]["route_id"].clone(),
            send["result"]["status"].clone(),
            send["result"]["attempts"][0]["status"].clone(),
            send["result"]["attempts"][1]["status"].clone(),
        ),
        (
            json!("delivery"),
            json!("email"),
            resolved["result"]["route_id"].clone(),
            json!("sent"),
            json!("transient_failure"),
            json!("sent"),
        ),
        "CHOKE-107: routing resolution is concretely handed off to delivery with deterministic retry->send semantics"
    );
}

#[test]
#[ignore]
fn choke_108_gating_to_approval_flow_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery();
    let _spawn_delivery = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-108-spawn","method":"meta/spawn_member","params":{"module_id":"delivery"}}"#,
        Duration::from_secs(1),
    ));
    let evaluated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"choke-108-evaluate",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"deploy_prod_router",
                "actor_id":"alice",
                "risk_tier":"r3",
                "requested_approver":"bob",
                "approval_recipient":"approvals@example.com",
                "approval_channel":"transactional",
                "approval_timeout_ms":60_000
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-108-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let self_approve = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"choke-108-self-approve",
            "method":"meta/gating/decide",
            "params":{
                "pending_id": evaluated["result"]["pending_id"].clone(),
                "approver_id":"alice",
                "decision":"approve"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let approved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"choke-108-approve",
            "method":"meta/gating/decide",
            "params":{
                "pending_id": evaluated["result"]["pending_id"].clone(),
                "approver_id":"bob",
                "decision":"approve",
                "reason":"peer-reviewed"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let audit = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-108-audit","method":"meta/gating/audit","params":{"limit":10}}"#,
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-108-history","method":"meta/delivery/history","params":{"recipient":"approvals@example.com","limit":5}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    let entries = audit["result"]["entries"]
        .as_array()
        .expect("audit entries");
    let event_types = entries
        .iter()
        .filter_map(|entry| entry.get("event_type").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let pending_created = entries
        .iter()
        .find(|entry| entry.get("event_type") == Some(&json!("pending_created")))
        .expect("pending_created entry");
    let approval_decided = entries
        .iter()
        .find(|entry| entry.get("event_type") == Some(&json!("approval_decided")))
        .expect("approval_decided entry");
    assert_eq!(evaluated["result"]["risk_tier"], json!("r3"));
    assert_eq!(evaluated["result"]["outcome"], json!("pending_approval"));
    assert_eq!(
        pending["result"]["pending"][0]["requested_approver"],
        json!("bob")
    );
    assert_eq!(
        self_approve["error"]["message"],
        json!("Invalid params: approver_id cannot self-approve the action actor")
    );
    assert_eq!(approved["result"]["outcome"], json!("allowed"));
    assert_eq!(approved["result"]["approver_id"], json!("bob"));
    assert!(event_types.contains(&"pending_created"));
    assert!(event_types.contains(&"approval_decided"));
    assert_eq!(
        pending_created["detail"]["approval_route_id"],
        pending["result"]["pending"][0]["approval_route_id"]
    );
    assert_eq!(
        pending_created["detail"]["approval_delivery_id"],
        pending["result"]["pending"][0]["approval_delivery_id"]
    );
    assert_eq!(
        approval_decided["detail"]["approval_route_id"],
        pending["result"]["pending"][0]["approval_route_id"]
    );
    assert_eq!(
        approval_decided["detail"]["approval_delivery_id"],
        pending["result"]["pending"][0]["approval_delivery_id"]
    );
    assert_eq!(
        history["result"]["deliveries"][0]["route_id"],
        pending["result"]["pending"][0]["approval_route_id"]
    );
    assert_eq!(
        history["result"]["deliveries"][0]["delivery_id"],
        pending["result"]["pending"][0]["approval_delivery_id"]
    );
}

#[test]
#[ignore]
fn choke_109_memory_to_gating_conflict_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery();
    let indexed = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"choke-109-memory-index",
            "method":"meta/memory/index",
            "params":{
                "entity":"router",
                "topic":"deploy",
                "fact":"unsafe_state_observed",
                "metadata":{"source":"memory-module","confidence":"high"},
                "conflict":true,
                "conflict_reason":"assertion_conflict"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let queried = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-109-memory-query","method":"meta/memory/query","params":{"entity":"router","topic":"deploy"}}"#,
        Duration::from_secs(1),
    ));
    let gated = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"choke-109-gating",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"deploy_prod_router",
                "actor_id":"alice",
                "risk_tier":"r2",
                "entity":"router",
                "topic":"deploy"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let audit = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-109-audit","method":"meta/gating/audit","params":{"limit":20}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    let entries = audit["result"]["entries"]
        .as_array()
        .expect("audit entries");
    let conflict_blocked = entries
        .iter()
        .find(|entry| entry.get("event_type") == Some(&json!("conflict_blocked")))
        .expect("conflict_blocked audit entry");
    assert_eq!(
        (
            indexed["result"]["assertion_id"].is_string(),
            indexed["result"]["conflict_active"].clone(),
            queried["result"]["assertions"][0]["fact"].clone(),
            queried["result"]["conflicts"][0]["reason"].clone(),
            gated["result"]["outcome"].clone(),
            gated["result"]["fallback_reason"].clone(),
            gated["result"]["pending_id"].clone(),
            conflict_blocked["detail"]["reason"].clone(),
            conflict_blocked["detail"]["conflict"]["entity"].clone(),
            conflict_blocked["detail"]["conflict"]["topic"].clone(),
        ),
        (
            true,
            json!(true),
            json!("unsafe_state_observed"),
            json!("assertion_conflict"),
            json!("safe_draft"),
            json!("memory_conflict"),
            Value::Null,
            json!("memory_conflict"),
            json!("router"),
            json!("deploy"),
        ),
        "CHOKE-109: memory assertions index/query conflict signal must concretely block R2 gating and emit conflict-blocked audit"
    );
}

#[test]
#[ignore]
fn choke_110_sdk_contract_mapping_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery();
    let unloaded = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-110-unloaded","method":"unknown/route","params":{}}"#,
        Duration::from_secs(1),
    ));
    let invalid_params = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"choke-110-invalid","method":"meta/spawn_member","params":{}}"#,
        Duration::from_secs(1),
    ));

    let ts_unloaded = map_ts_sdk_response(&unloaded);
    let py_unloaded = map_python_sdk_response(&unloaded);
    let ts_invalid = map_ts_sdk_response(&invalid_params);
    let py_invalid = map_python_sdk_response(&invalid_params);

    runtime.shutdown();

    let state = decision_state(true);
    let token = trusted_token(
        json!({
            "sub":"user-110",
            "email":"alice@example.com",
            "iss":"https://trusted.meta.localhost",
            "aud":"meerkat-console",
            "provider":"google_oauth",
            "exp":4_000_000_000_u64
        }),
        "kid-current",
        "phase7-trusted-current-secret",
    );
    let sdk_auth = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!("/console/modules?auth_token={token}"),
            auth: None,
        },
    );
    let jwt_local = validate_jwt_locally(
        &token,
        &JwtValidationConfig {
            shared_secret: "phase7-trusted-current-secret".to_string(),
            issuer: Some("https://trusted.meta.localhost".to_string()),
            audience: Some("meerkat-console".to_string()),
            now_epoch_seconds: 1_900_000_000,
            leeway_seconds: 30,
        },
    )
    .expect("local jwt validation should succeed without runtime process boundary");

    assert_eq!(
        (
            ts_unloaded,
            py_unloaded,
            ts_invalid,
            py_invalid,
            sdk_auth.status,
            jwt_local.email.as_deref(),
            jwt_local.subject.as_deref()
        ),
        (
            SdkMappedOutcome::Error {
                id: json!("choke-110-unloaded"),
                code: -32601,
                message: "Module 'unknown' not loaded".to_string(),
            },
            SdkMappedOutcome::Error {
                id: json!("choke-110-unloaded"),
                code: -32601,
                message: "Module 'unknown' not loaded".to_string(),
            },
            SdkMappedOutcome::Error {
                id: json!("choke-110-invalid"),
                code: -32602,
                message: "Invalid params: module_id required".to_string(),
            },
            SdkMappedOutcome::Error {
                id: json!("choke-110-invalid"),
                code: -32602,
                message: "Invalid params: module_id required".to_string(),
            },
            200,
            Some("alice@example.com"),
            Some("user-110")
        ),
        "CHOKE-110: preserves RPC error shape and proves SDK local JWT auth chokepoint is runtime-local with no process-boundary dependency"
    );
}

#[test]
#[ignore]
fn e2e_401_rpc_surface_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery_mcp();
    let status = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-401-status","method":"meta/status","params":{}}"#,
        Duration::from_secs(1),
    ));
    let reconcile = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-401-reconcile","method":"meta/reconcile","params":{"modules":["router","delivery"]}}"#,
        Duration::from_secs(1),
    ));
    let resolve = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-401-resolve","method":"meta/routing/resolve","params":{"recipient":"hi@example.com","channel":"transactional"}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        json!({"status":status,"resolve":resolve,"reconcile":reconcile}),
        json!({
            "status":{
                "jsonrpc":"2.0",
                "id":"e2e-401-status",
                "result":{
                    "contract_version":"0.5.0",
                    "running":true,
                    "loaded_modules":["router"]
                }
            },
            "resolve":{
                "jsonrpc":"2.0",
                "id":"e2e-401-resolve",
                "result":{
                    "route_id":"route-000000",
                    "recipient":"hi@example.com",
                    "channel":"transactional",
                    "sink":"email",
                    "target_module":"delivery",
                    "retry_max":1,
                    "backoff_ms":250,
                    "rate_limit_per_minute":2
                }
            },
            "reconcile":{
                "jsonrpc":"2.0",
                "id":"e2e-401-reconcile",
                "result":{
                    "accepted":true,
                    "reconciled_modules":["router","delivery"],
                    "added":1
                }
            }
        }),
        "E2E-401: end-to-end RPC interaction verified"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_501_session_persistence_target_defined_red() {
    let state = decision_state(true);
    let contracts = session_store_contracts(&state);
    let temp = tempdir().expect("tempdir");
    let sessions_path = temp.path().join("sessions.json");

    let writes = vec![
        SessionPersistenceRow {
            session_id: "s1".to_string(),
            updated_at_ms: 1_000,
            deleted: false,
            payload: json!({"step":"create"}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "s1".to_string(),
            updated_at_ms: 2_000,
            deleted: true,
            payload: json!({}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "s2".to_string(),
            updated_at_ms: 1_500,
            deleted: false,
            payload: json!({"step":"create"}),
            ..Default::default()
        },
        SessionPersistenceRow {
            session_id: "s2".to_string(),
            updated_at_ms: 3_000,
            deleted: false,
            payload: json!({"step":"update","version":2}),
            ..Default::default()
        },
    ];
    // read_live_rows uses server-side QUALIFY dedup + deleted=false filter
    let live_query_rows = json!({
        "rows": [
            {"f":[{"v":"s2"},{"v":"3000"},{"v":"false"},{"v":"{\"step\":\"update\",\"version\":2}"}]}
        ]
    });
    let bq_server = MockHttpServer::start(vec![
        MockHttpResponse::json(json!({})),
        MockHttpResponse::json(live_query_rows),
    ]);

    let json_store = JsonFileSessionStore::new(&sessions_path)
        .with_stale_lock_threshold(Duration::from_millis(1));
    json_store
        .append_rows(&writes)
        .expect("json-file backend writes rows");
    let latest = json_store
        .read_latest_rows()
        .expect("json-file backend reads latest rows");
    let live = json_store
        .read_live_rows()
        .expect("json-file backend reads live rows");

    let bq_store = BigQuerySessionStoreAdapter::new_native(
        state.bigquery.dataset.clone(),
        state.bigquery.table.clone(),
    )
    .with_project_id("phase3c-project")
    .with_access_token("phase3c-token")
    .with_api_base_url(format!("{}/bigquery/v2", bq_server.base_url()));
    bq_store
        .stream_insert_rows(&writes)
        .await
        .expect("bigquery adapter issues insert command");
    let bq_live = bq_store
        .read_live_rows()
        .await
        .expect("bigquery adapter reads live rows through query path");
    let bq_requests = bq_server.captured_requests();
    assert_eq!(
        bq_requests.len(),
        2,
        "expected one insertAll call and one query call"
    );
    let bq_insert_body: Value =
        serde_json::from_str(&bq_requests[0].body).expect("parse insertAll request body");
    assert_eq!(
        bq_insert_body["rows"].as_array().map_or(0, Vec::len),
        writes.len()
    );
    assert!(
        bq_requests[0]
            .path
            .contains("/datasets/phase3c_dataset/tables/phase3c_table/insertAll"),
        "insert request should include concrete dataset.table path"
    );
    let bq_query_body: Value =
        serde_json::from_str(&bq_requests[1].body).expect("parse query request body");
    let bq_query_text = bq_query_body["query"]
        .as_str()
        .expect("query text should be present");
    assert!(
        bq_query_text.contains("SELECT session_id, updated_at_ms, deleted, payload, labels_json"),
        "query request should include concrete read-path SQL"
    );
    assert!(
        bq_query_text.contains("phase3c-project.phase3c_dataset.phase3c_table"),
        "query request should reference project.dataset.table"
    );

    assert_eq!(
        json!({
            "stores": contracts
                .iter()
                .map(|contract| {
                    json!({
                        "store": contract.store,
                        "dedup": contract.dedup_read_path,
                        "tombstones": contract.tombstones_supported
                    })
                })
                .collect::<Vec<_>>(),
            "latest": latest,
            "live": live,
            "bq_live": bq_live
        }),
        json!({
            "stores": [
                {"store":"big_query","dedup":true,"tombstones":true},
                {"store":"json_file","dedup":true,"tombstones":true}
            ],
            "latest": [
                {"session_id":"s1","updated_at_ms":2000,"deleted":true,"payload":{},"labels":{}},
                {"session_id":"s2","updated_at_ms":3000,"deleted":false,"payload":{"step":"update","version":2},"labels":{}}
            ],
            "live": [
                {"session_id":"s2","updated_at_ms":3000,"deleted":false,"payload":{"step":"update","version":2},"labels":{}}
            ],
            "bq_live": [
                {"session_id":"s2","updated_at_ms":3000,"deleted":false,"payload":{"step":"update","version":2},"labels":{}}
            ]
        }),
        "E2E-501: concrete JSON + BigQuery backend paths preserve dedup-by-latest-row and tombstone filtering semantics"
    );
}

#[test]
#[ignore]
fn e2e_601_sse_experience_target_defined_red() {
    let mut runtime = runtime_with_phase6_agent_events();
    let first_subscribe = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-first","method":"meta/events/subscribe","params":{"scope":"mob"}}"#,
        Duration::from_secs(1),
    ));
    let spawn_delivery = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-spawn","method":"meta/spawn_member","params":{"module_id":"delivery"}}"#,
        Duration::from_secs(1),
    ));
    let second_subscribe = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-second","method":"meta/events/subscribe","params":{"scope":"mob","last_event_id":"evt-agent-a-2"}}"#,
        Duration::from_secs(1),
    ));
    let agent_scope_subscribe = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-agent","method":"meta/events/subscribe","params":{"scope":"agent","agent_id":"agent-alpha"}}"#,
        Duration::from_secs(1),
    ));
    let agent_checkpoint_subscribe = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-agent-checkpoint","method":"meta/events/subscribe","params":{"scope":"agent","agent_id":"agent-alpha","last_event_id":"evt-agent-a-2"}}"#,
        Duration::from_secs(1),
    ));
    let interaction_scope_subscribe = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-interaction","method":"meta/events/subscribe","params":{"scope":"interaction"}}"#,
        Duration::from_secs(1),
    ));
    let invalid_scope = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-invalid-scope","method":"meta/events/subscribe","params":{"scope":"tenant"}}"#,
        Duration::from_secs(1),
    ));
    let invalid_checkpoint = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-invalid-checkpoint","method":"meta/events/subscribe","params":{"scope":"mob","last_event_id":"evt-missing"}}"#,
        Duration::from_secs(1),
    ));
    let bounded_checkpoint = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-bounded-checkpoint","method":"meta/events/subscribe","params":{"scope":"mob","last_event_id":"evt-router"}}"#,
        Duration::from_secs(1),
    ));
    let missing_agent_id = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-missing-agent-id","method":"meta/events/subscribe","params":{"scope":"agent"}}"#,
        Duration::from_secs(1),
    ));
    let empty_agent_id = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-601-empty-agent-id","method":"meta/events/subscribe","params":{"scope":"agent","agent_id":""}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    let first_events = subscribed_events(&first_subscribe);
    let second_events = subscribed_events(&second_subscribe);
    let agent_events = subscribed_events(&agent_scope_subscribe);
    let agent_checkpoint_events = subscribed_events(&agent_checkpoint_subscribe);
    let first_ids: Vec<&str> = first_events
        .iter()
        .map(|event| {
            event
                .get("event_id")
                .and_then(Value::as_str)
                .expect("all events should have event_id")
        })
        .collect();
    let second_ids: Vec<&str> = second_events
        .iter()
        .map(|event| {
            event
                .get("event_id")
                .and_then(Value::as_str)
                .expect("all events should have event_id")
        })
        .collect();
    let agent_ids: Vec<&str> = agent_events
        .iter()
        .map(|event| {
            event
                .get("event_id")
                .and_then(Value::as_str)
                .expect("all events should have event_id")
        })
        .collect();
    let checkpoint_agent_ids: Vec<&str> = agent_checkpoint_events
        .iter()
        .map(|event| {
            event
                .get("event_id")
                .and_then(Value::as_str)
                .expect("all events should have event_id")
        })
        .collect();
    let checkpoint_index = second_ids
        .iter()
        .position(|id| *id == "evt-agent-a-2")
        .expect("second subscribe should contain reconnect checkpoint id");
    let reconnect_backfill = second_ids[(checkpoint_index + 1)..].to_vec();
    let first_keep_alive = subscribed_keep_alive(&first_subscribe);
    let second_keep_alive = subscribed_keep_alive(&second_subscribe);
    let first_keep_alive_comment = subscribed_keep_alive_comment(&first_subscribe);
    let second_keep_alive_comment = subscribed_keep_alive_comment(&second_subscribe);
    let first_frames = subscribed_event_frames(&first_subscribe);
    let second_frames = subscribed_event_frames(&second_subscribe);
    let agent_frames = subscribed_event_frames(&agent_scope_subscribe);
    let interaction_keep_alive = subscribed_keep_alive(&interaction_scope_subscribe);
    let interaction_events = subscribed_events(&interaction_scope_subscribe);
    let interaction_ids: Vec<&str> = interaction_events
        .iter()
        .map(|event| {
            event
                .get("event_id")
                .and_then(Value::as_str)
                .expect("all events should have event_id")
        })
        .collect();
    let agent_event_agent_ids: Vec<&str> = agent_events
        .iter()
        .map(|event| {
            event
                .get("event")
                .and_then(|value| value.get("agent_id"))
                .and_then(Value::as_str)
                .expect("agent scope must return agent events")
        })
        .collect();

    assert_eq!(
        json!({
            "spawn": spawn_delivery,
            "first_ids": first_ids,
            "second_ids": second_ids,
            "reconnect_backfill": reconnect_backfill,
            "agent_ids": agent_ids,
            "agent_checkpoint_ids": checkpoint_agent_ids,
            "agent_event_agent_ids": agent_event_agent_ids,
            "first_keep_alive": first_keep_alive,
            "second_keep_alive": second_keep_alive,
            "first_keep_alive_comment": first_keep_alive_comment,
            "second_keep_alive_comment": second_keep_alive_comment,
            "first_frames": first_frames,
            "second_frames": second_frames,
            "agent_frames": agent_frames,
            "interaction_scope": interaction_scope_subscribe["result"]["scope"],
            "interaction_ids": interaction_ids,
            "interaction_keep_alive": interaction_keep_alive,
            "invalid_scope": invalid_scope,
            "invalid_checkpoint": invalid_checkpoint,
            "bounded_checkpoint": bounded_checkpoint,
            "missing_agent_id": missing_agent_id,
            "empty_agent_id": empty_agent_id
        }),
        json!({
            "spawn": {
                "jsonrpc":"2.0",
                "id":"e2e-601-spawn",
                "result":{
                    "accepted":true,
                    "module_id":"delivery"
                }
            },
            "first_ids": ["evt-agent-b-1","evt-agent-a-2","evt-agent-a-3"],
            "second_ids": ["evt-agent-a-2","evt-agent-a-3","evt-delivery"],
            "reconnect_backfill": ["evt-agent-a-3","evt-delivery"],
            "agent_ids": ["evt-agent-a-1","evt-agent-a-2","evt-agent-a-3"],
            "agent_checkpoint_ids": ["evt-agent-a-2","evt-agent-a-3"],
            "agent_event_agent_ids": ["agent-alpha","agent-alpha","agent-alpha"],
            "first_keep_alive": {
                "interval_ms":15000,
                "event":"keep-alive"
            },
            "second_keep_alive": {
                "interval_ms":15000,
                "event":"keep-alive"
            },
            "first_keep_alive_comment":": keep-alive\n\n",
            "second_keep_alive_comment":": keep-alive\n\n",
            "first_frames": [
                "id: evt-agent-b-1\nevent: tick\ndata: {\"kind\":\"agent\",\"agent_id\":\"agent-beta\",\"event_type\":\"tick\"}\n\n",
                "id: evt-agent-a-2\nevent: tick\ndata: {\"kind\":\"agent\",\"agent_id\":\"agent-alpha\",\"event_type\":\"tick\"}\n\n",
                "id: evt-agent-a-3\nevent: interaction.reply\ndata: {\"kind\":\"agent\",\"agent_id\":\"agent-alpha\",\"event_type\":\"interaction.reply\"}\n\n"
            ],
            "second_frames": [
                "id: evt-agent-a-2\nevent: tick\ndata: {\"kind\":\"agent\",\"agent_id\":\"agent-alpha\",\"event_type\":\"tick\"}\n\n",
                "id: evt-agent-a-3\nevent: interaction.reply\ndata: {\"kind\":\"agent\",\"agent_id\":\"agent-alpha\",\"event_type\":\"interaction.reply\"}\n\n",
                "id: evt-delivery\nevent: ready\ndata: {\"kind\":\"module\",\"module\":\"delivery\",\"event_type\":\"ready\",\"payload\":{\"sink\":\"memory\"}}\n\n"
            ],
            "agent_frames": [
                "id: evt-agent-a-1\nevent: tick\ndata: {\"kind\":\"agent\",\"agent_id\":\"agent-alpha\",\"event_type\":\"tick\"}\n\n",
                "id: evt-agent-a-2\nevent: tick\ndata: {\"kind\":\"agent\",\"agent_id\":\"agent-alpha\",\"event_type\":\"tick\"}\n\n",
                "id: evt-agent-a-3\nevent: interaction.reply\ndata: {\"kind\":\"agent\",\"agent_id\":\"agent-alpha\",\"event_type\":\"interaction.reply\"}\n\n"
            ],
            "interaction_scope":"interaction",
            "interaction_ids": ["evt-agent-a-0","evt-agent-b-0","evt-agent-a-3"],
            "interaction_keep_alive": {
                "interval_ms":15000,
                "event":"keep-alive"
            },
            "invalid_scope": {
                "jsonrpc":"2.0",
                "id":"e2e-601-invalid-scope",
                "error":{
                    "code":-32602,
                    "message":"Invalid params: unsupported scope 'tenant' (allowed: mob, agent, interaction)"
                }
            },
            "invalid_checkpoint": {
                "jsonrpc":"2.0",
                "id":"e2e-601-invalid-checkpoint",
                "error":{
                    "code":-32602,
                    "message":"Invalid params: unknown last_event_id 'evt-missing'"
                }
            },
            "bounded_checkpoint": {
                "jsonrpc":"2.0",
                "id":"e2e-601-bounded-checkpoint",
                "error":{
                    "code":-32602,
                    "message":"Invalid params: unknown last_event_id 'evt-router'"
                }
            },
            "missing_agent_id": {
                "jsonrpc":"2.0",
                "id":"e2e-601-missing-agent-id",
                "error":{
                    "code":-32602,
                    "message":"Invalid params: agent_id is required when scope is 'agent'"
                }
            },
            "empty_agent_id": {
                "jsonrpc":"2.0",
                "id":"e2e-601-empty-agent-id",
                "error":{
                    "code":-32602,
                    "message":"Invalid params: agent_id cannot be empty when scope is 'agent'"
                }
            }
        }),
        "E2E-601: subscribe snapshot + checkpoint replay model covers SSE reconnect/backfill semantics with deterministic ordering"
    );
}

#[test]
#[ignore]
fn e2e_701_auth_flow_target_defined_red() {
    let state = decision_state(true);
    let user_token = trusted_token(
        json!({
            "sub":"user-1",
            "email":"alice@example.com",
            "iss":"https://trusted.meta.localhost",
            "aud":"meerkat-console",
            "provider":"google_oauth",
            "exp":4_000_000_000_u64
        }),
        "kid-current",
        "phase7-trusted-current-secret",
    );
    let service_token = trusted_token(
        json!({
            "sub":"svc:deploy-bot",
            "actor_type":"service",
            "iss":"https://trusted.meta.localhost",
            "aud":"meerkat-console",
            "provider":"generic_oidc",
            "exp":4_000_000_000_u64
        }),
        "kid-next",
        "phase7-trusted-next-secret",
    );
    let injected_discovery = b64_json(json!({
        "issuer":"https://attacker.example",
        "jwks_uri":"https://attacker.example/jwks"
    }));
    let injected_jwks = b64_json(json!({
        "keys":[{"kid":"attacker","kty":"oct","alg":"HS256","k":URL_SAFE_NO_PAD.encode("attacker-secret".as_bytes())}]
    }));
    let allowed = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!(
                "/console/modules?auth_token={}&oidc_discovery_b64={}&jwks_b64={}&audience=attacker-aud",
                user_token, injected_discovery, injected_jwks
            ),
            auth: None,
        },
    );
    let service_allowed = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: format!("/console/modules?auth_token={service_token}"),
            auth: None,
        },
    );
    let denied_allowlist = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: Some(ConsoleAccessRequest {
                provider: AuthProvider::GoogleOAuth,
                email: "mallory@example.com".to_string(),
            }),
        },
    );
    let denied_service = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/modules".to_string(),
            auth: Some(ConsoleAccessRequest {
                provider: AuthProvider::ServiceIdentity,
                email: "svc:unknown".to_string(),
            }),
        },
    );

    assert_eq!(
        (
            allowed.status,
            service_allowed.status,
            denied_allowlist.status,
            denied_allowlist.body,
            denied_service.status,
            denied_service.body
        ),
        (
            200,
            200,
            401,
            json!({"error":"unauthorized","reason":"email_not_allowlisted"}),
            401,
            json!({"error":"unauthorized","reason":"service_identity_not_allowlisted"})
        ),
        "E2E-701: token-based protected-console auth validates OIDC/JWKS JWTs and enforces user/service identity allowlist constraints"
    );
}

#[test]
#[ignore]
fn e2e_801_console_experience_target_defined_red() {
    let state = decision_state(false);
    let open = handle_console_rest_json_route(
        &state,
        &ConsoleRestJsonRequest {
            method: "GET".to_string(),
            path: "/console/experience".to_string(),
            auth: None,
        },
    );

    assert_eq!(open.status, 200);
    assert_eq!(
        (
            open.body["contract_version"].clone(),
            open.body["base_panel"]["panel_id"].clone(),
            open.body["base_panel"]["route"].clone(),
            open.body["module_panels"].clone(),
            open.body["activity_feed"]["source_route"].clone(),
            open.body["activity_feed"]["request_contract"]["last_event_id_header"].clone(),
            open.body["activity_feed"]["keep_alive"]["event"].clone()
        ),
        (
            json!("0.5.0"),
            json!("console.home"),
            json!("/console/experience"),
            json!([
                {
                    "panel_id":"module.router",
                    "module_id":"router",
                    "title":"router module",
                    "route":"/console/modules/router",
                    "capabilities":{"can_render":true,"can_subscribe_activity":true}
                },
                {
                    "panel_id":"module.delivery",
                    "module_id":"delivery",
                    "title":"delivery module",
                    "route":"/console/modules/delivery",
                    "capabilities":{"can_render":true,"can_subscribe_activity":true}
                }
            ]),
            json!("/console/timeline/stream"),
            json!("optional Last-Event-ID checkpoint from prior event_id"),
            json!("keep-alive")
        ),
        "E2E-801: console route emits concrete capability-driven base/module panel schema and the unified timeline feed contract"
    );
}

#[test]
#[ignore]
fn e2e_1001_routing_delivery_flow_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery_mcp();
    let _spawn_delivery = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1001-spawn","method":"meta/spawn_member","params":{"module_id":"delivery"}}"#,
        Duration::from_secs(1),
    ));
    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1001-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com"}}"#,
        Duration::from_secs(1),
    ));
    let send_first = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"e2e-1001-send-a",
            "method":"meta/delivery/send",
            "params":{
                "resolution": resolved["result"].clone(),
                "idempotency_key":"e2e-1001-key",
                "payload":{"message":"hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let send_second = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"e2e-1001-send-b",
            "method":"meta/delivery/send",
            "params":{
                "resolution": resolved["result"].clone(),
                "idempotency_key":"e2e-1001-key",
                "payload":{"message":"hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1001-history","method":"meta/delivery/history","params":{"recipient":"user@example.com","limit":5}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        (
            resolved["result"]["sink"].clone(),
            send_first["result"]["status"].clone(),
            send_first["result"]["delivery_id"].clone(),
            send_second["result"]["delivery_id"].clone(),
            history["result"]["deliveries"][0]["route_id"].clone(),
            history["result"]["deliveries"][0]["status"].clone(),
            history["result"]["deliveries"]
                .as_array()
                .map_or(0, Vec::len),
        ),
        (
            json!("email"),
            json!("sent"),
            send_first["result"]["delivery_id"].clone(),
            send_first["result"]["delivery_id"].clone(),
            resolved["result"]["route_id"].clone(),
            json!("sent"),
            1,
        ),
        "E2E-1001: route resolution->delivery send is wired end-to-end with idempotent replay and persisted delivery history"
    );
}

#[test]
#[ignore]
fn e2e_1101_sdk_parity_flow_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery();

    let status = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1101-status","method":"meta/status","params":{}}"#,
        Duration::from_secs(1),
    ));
    let caps = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1101-caps","method":"meta/capabilities","params":{}}"#,
        Duration::from_secs(1),
    ));
    let invalid = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1101-invalid","method":"meta/spawn_member","params":{}}"#,
        Duration::from_secs(1),
    ));
    let unloaded = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1101-unloaded","method":"delivery/tools.list","params":{"probe":"parity"}}"#,
        Duration::from_secs(1),
    ));

    let ts_flow = vec![
        map_ts_sdk_response(&status),
        map_ts_sdk_response(&caps),
        map_ts_sdk_response(&invalid),
        map_ts_sdk_response(&unloaded),
    ];
    let py_flow = vec![
        map_python_sdk_response(&status),
        map_python_sdk_response(&caps),
        map_python_sdk_response(&invalid),
        map_python_sdk_response(&unloaded),
    ];

    runtime.shutdown();

    let expected_capability_methods = json!([
        "meta/status",
        "meta/capabilities",
        "meta/reconcile",
        "meta/spawn_member",
        "meta/routing/resolve",
        "meta/routing/routes/list",
        "meta/routing/routes/add",
        "meta/routing/routes/delete",
        "meta/delivery/send",
        "meta/delivery/history",
        "meta/events/subscribe",
        "meta/memory/stores",
        "meta/memory/index",
        "meta/memory/query",
        "meta/session_store/bigquery",
        "meta/gating/evaluate",
        "meta/gating/pending",
        "meta/gating/decide",
        "meta/gating/audit",
        "meta/call_tool",
        "meta/models/catalog"
    ]);

    assert_eq!(
        (ts_flow, py_flow),
        (
            vec![
                SdkMappedOutcome::Success {
                    id: json!("e2e-1101-status"),
                    result: json!({
                        "contract_version":"0.5.0",
                        "running":true,
                        "loaded_modules":["router"]
                    }),
                },
                SdkMappedOutcome::Success {
                    id: json!("e2e-1101-caps"),
                    result: json!({
                        "contract_version":"0.5.0",
                        "methods": expected_capability_methods.clone(),
                        "loaded_modules":["router"],
                        "runtime_capabilities": {
                            "can_spawn_members": false,
                            "can_send_messages": false,
                            "can_wire_members": false,
                            "can_retire_members": false,
                            "available_spawn_modes": ["module"],
                        }
                    }),
                },
                SdkMappedOutcome::Error {
                    id: json!("e2e-1101-invalid"),
                    code: -32602,
                    message: "Invalid params: module_id required".to_string(),
                },
                SdkMappedOutcome::Error {
                    id: json!("e2e-1101-unloaded"),
                    code: -32601,
                    message: "Module 'delivery' not loaded".to_string(),
                },
            ],
            vec![
                SdkMappedOutcome::Success {
                    id: json!("e2e-1101-status"),
                    result: json!({
                        "contract_version":"0.5.0",
                        "running":true,
                        "loaded_modules":["router"]
                    }),
                },
                SdkMappedOutcome::Success {
                    id: json!("e2e-1101-caps"),
                    result: json!({
                        "contract_version":"0.5.0",
                        "methods": expected_capability_methods,
                        "loaded_modules":["router"],
                        "runtime_capabilities": {
                            "can_spawn_members": false,
                            "can_send_messages": false,
                            "can_wire_members": false,
                            "can_retire_members": false,
                            "available_spawn_modes": ["module"],
                        }
                    }),
                },
                SdkMappedOutcome::Error {
                    id: json!("e2e-1101-invalid"),
                    code: -32602,
                    message: "Invalid params: module_id required".to_string(),
                },
                SdkMappedOutcome::Error {
                    id: json!("e2e-1101-unloaded"),
                    code: -32601,
                    message: "Module 'delivery' not loaded".to_string(),
                },
            ],
        ),
        "E2E-1101: end-to-end TS/Python SDK parity flow proves shared core-method contracts and compatible invalid-params/unloaded-module error shapes"
    );
}

#[test]
#[ignore]
fn e2e_1201_gating_flow_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery();
    let _spawn_delivery = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1201-spawn","method":"meta/spawn_member","params":{"module_id":"delivery"}}"#,
        Duration::from_secs(1),
    ));
    let r2 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"e2e-1201-r2",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"notify_customer",
                "actor_id":"ops-bot",
                "risk_tier":"r2",
                "rationale":"consequence mode"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let r3_delivery = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"e2e-1201-r3-delivery",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"rotate_prod_secret",
                "actor_id":"ops-bot",
                "risk_tier":"r3",
                "requested_approver":"lead-approver",
                "approval_recipient":"approvals@example.com",
                "approval_channel":"transactional",
                "approval_timeout_ms":60_000
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending_before_approve = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1201-pending-before-approve","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let _approved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"e2e-1201-approve",
            "method":"meta/gating/decide",
            "params":{
                "pending_id": r3_delivery["result"]["pending_id"].clone(),
                "approver_id":"lead-approver",
                "decision":"approve"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let _r3_timeout = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"e2e-1201-r3",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"delete_prod_data",
                "actor_id":"mallory",
                "risk_tier":"r3",
                "approval_timeout_ms":0
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending_after_timeout = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1201-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let audit = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1201-audit","method":"meta/gating/audit","params":{"limit":20}}"#,
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1201-history","method":"meta/delivery/history","params":{"recipient":"approvals@example.com","limit":5}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    let entries = audit["result"]["entries"]
        .as_array()
        .expect("audit entries");
    let has_timeout_fallback = entries.iter().any(|entry| {
        entry.get("event_type") == Some(&json!("timeout_fallback"))
            && entry.get("outcome") == Some(&json!("safe_draft"))
    });
    let has_r2_consequence_audit = entries.iter().any(|entry| {
        entry.get("outcome") == Some(&json!("allowed_with_audit"))
            && entry.get("detail").and_then(|detail| detail.get("policy"))
                == Some(&json!("consequence_mode_allow_with_audit_v0_1"))
    });
    let pending_created_delivery = entries
        .iter()
        .find(|entry| {
            entry.get("event_type") == Some(&json!("pending_created"))
                && entry.get("detail").and_then(|detail| detail.get("action"))
                    == Some(&json!("rotate_prod_secret"))
        })
        .expect("pending_created entry for delivery-routed r3 action");
    let approval_decided_delivery = entries
        .iter()
        .find(|entry| {
            entry.get("event_type") == Some(&json!("approval_decided"))
                && entry
                    .get("detail")
                    .and_then(|detail| detail.get("approver_id"))
                    == Some(&json!("lead-approver"))
        })
        .expect("approval_decided entry for delivery-routed r3 action");

    assert_eq!(
        (
            r2["result"]["risk_tier"].clone(),
            r2["result"]["outcome"].clone(),
            pending_after_timeout["result"]["pending"].clone(),
            has_timeout_fallback,
            has_r2_consequence_audit,
            pending_created_delivery["detail"]["approval_route_id"].clone(),
            pending_created_delivery["detail"]["approval_delivery_id"].clone(),
            approval_decided_delivery["detail"]["approval_route_id"].clone(),
            approval_decided_delivery["detail"]["approval_delivery_id"].clone(),
            history["result"]["deliveries"][0]["route_id"].clone(),
            history["result"]["deliveries"][0]["delivery_id"].clone(),
        ),
        (
            json!("r2"),
            json!("allowed_with_audit"),
            json!([]),
            true,
            true,
            pending_before_approve["result"]["pending"][0]["approval_route_id"].clone(),
            pending_before_approve["result"]["pending"][0]["approval_delivery_id"].clone(),
            pending_before_approve["result"]["pending"][0]["approval_route_id"].clone(),
            pending_before_approve["result"]["pending"][0]["approval_delivery_id"].clone(),
            pending_before_approve["result"]["pending"][0]["approval_route_id"].clone(),
            pending_before_approve["result"]["pending"][0]["approval_delivery_id"].clone(),
        ),
        "E2E-1201: end-to-end gating wires R2 consequence-mode allow-with-audit, R3 delivery-path approval linkage, and R3 timeout fallback to safe-draft with auditable records"
    );
}

#[test]
#[ignore]
fn e2e_1301_memory_gating_flow_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery();
    let stores_before = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1301-stores-before","method":"meta/memory/stores","params":{}}"#,
        Duration::from_secs(1),
    ));
    let _index_fact = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"e2e-1301-memory-index-fact",
            "method":"meta/memory/index",
            "params":{
                "entity":"delivery",
                "topic":"email_send",
                "fact":"pending_user_report"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let _index_conflict = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"e2e-1301-memory-index-conflict",
            "method":"meta/memory/index",
            "params":{
                "entity":"delivery",
                "topic":"email_send",
                "conflict":true,
                "conflict_reason":"high_risk_unverified_claim"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let blocked_r3 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"e2e-1301-gating-blocked",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"send_incident_email",
                "actor_id":"oncall-bot",
                "risk_tier":"r3",
                "entity":"delivery",
                "topic":"email_send",
                "requested_approver":"lead-approver",
                "approval_recipient":"approvals@example.com",
                "approval_channel":"transactional"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let allowed_r3 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc":"2.0",
            "id":"e2e-1301-gating-non-conflict",
            "method":"meta/gating/evaluate",
            "params":{
                "action":"send_sms_fallback",
                "actor_id":"oncall-bot",
                "risk_tier":"r3",
                "entity":"delivery",
                "topic":"sms_send",
                "requested_approver":"lead-approver",
                "approval_recipient":"approvals@example.com",
                "approval_channel":"transactional"
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let pending = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1301-pending","method":"meta/gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    let stores_after = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1301-stores-after","method":"meta/memory/stores","params":{}}"#,
        Duration::from_secs(1),
    ));
    let queried = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1301-query","method":"meta/memory/query","params":{"entity":"delivery","topic":"email_send"}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        (
            store_record_count(&stores_before, "knowledge_graph"),
            store_record_count(&stores_before, "vector"),
            store_record_count(&stores_before, "timeline"),
            store_record_count(&stores_before, "todo"),
            store_record_count(&stores_before, "top_of_mind"),
        ),
        (Some(0), Some(0), Some(0), Some(0), Some(0))
    );
    assert_eq!(
        (
            blocked_r3["result"]["outcome"].clone(),
            blocked_r3["result"]["fallback_reason"].clone(),
            blocked_r3["result"]["pending_id"].clone(),
            allowed_r3["result"]["outcome"].clone(),
            pending["result"]["pending"].as_array().map(Vec::len),
        ),
        (
            json!("safe_draft"),
            json!("memory_conflict"),
            Value::Null,
            json!("pending_approval"),
            Some(1),
        )
    );
    assert_eq!(
        (
            store_record_count(&stores_after, "knowledge_graph"),
            store_record_count(&stores_after, "vector"),
            store_record_count(&stores_after, "timeline"),
            store_record_count(&stores_after, "todo"),
            store_record_count(&stores_after, "top_of_mind"),
            queried["result"]["assertions"][0]["fact"].clone(),
            queried["result"]["conflicts"][0]["reason"].clone(),
        ),
        (
            Some(2),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            json!("pending_user_report"),
            json!("high_risk_unverified_claim"),
        ),
        "E2E-1301: memory stores/index/query flow must block R3 on referenced conflicts while still allowing non-conflicted R3 approval path"
    );
}

#[test]
#[ignore]
fn e2e_1401_program_smoke_target_defined_red() {
    let mut runtime = runtime_with_router_and_delivery();
    let status = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1401-status","method":"meta/status","params":{}}"#,
        Duration::from_secs(1),
    ));
    let events = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"e2e-1401-events","method":"meta/events/subscribe","params":{}}"#,
        Duration::from_secs(1),
    ));
    let shutdown = runtime.shutdown();
    let event_frames = subscribed_event_frames(&events);
    let keep_alive_comment = subscribed_keep_alive_comment(&events);

    assert_eq!(
        (
            status,
            subscribed_events(&events),
            subscribed_keep_alive(&events),
            events["result"]["replay_from_event_id"].clone(),
            events["result"]["scope"].clone(),
            event_frames.len(),
            event_frames
                .first()
                .map(|frame| frame.contains("id: evt-router"))
                .unwrap_or(false),
            event_frames
                .first()
                .map(|frame| frame.contains("event: response"))
                .unwrap_or(false),
            keep_alive_comment,
            shutdown.terminated_modules,
            shutdown.orphan_processes,
        ),
        (
            json!({
                "jsonrpc":"2.0",
                "id":"e2e-1401-status",
                "result":{
                    "contract_version":"0.5.0",
                    "running":true,
                    "loaded_modules":["router"]
                }
            }),
            vec![json!({
                "event_id":"evt-router",
                "source":"module",
                "timestamp_ms":10,
                "event":{
                    "kind":"module",
                    "module":"router",
                    "event_type":"response",
                    "payload":{"via":"router","ok":true}
                }
            })],
            json!({"event":"keep-alive","interval_ms":15000}),
            Value::Null,
            json!("mob"),
            1,
            true,
            true,
            ": keep-alive\n\n".to_string(),
            vec!["router".to_string()],
            0,
        ),
        "E2E-1401: program-level smoke proves startup status, event subscription envelope/frame contract, and deterministic shutdown bookkeeping"
    );
}
