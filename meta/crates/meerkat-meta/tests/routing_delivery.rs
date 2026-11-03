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

use meerkat_meta::{
    DiscoverySpec, MetaConfig, ModuleConfig, PreSpawnData, RestartPolicy, UnifiedEvent,
    handle_meta_rpc_json, start_meta_runtime,
};
use serde_json::{Value, json};

const BOUNDARY_ENV_KEY: &str = "META_MODULE_BOUNDARY";
const BOUNDARY_ENV_VALUE_MCP: &str = "mcp";
const DELAY_ON_ENV: &str = "META_PHASE_C_DELAY_ON";
const DELAY_MS_ENV: &str = "META_PHASE_C_DELAY_MS";

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

fn routing_delivery_runtime_with_env(
    router_extra_env: &[(&str, &str)],
    delivery_extra_env: &[(&str, &str)],
) -> meerkat_meta::MetaRuntimeHandle {
    let fixture_binary = fixture_binary_path();
    let config = MetaConfig {
        modules: vec![
            fixture_module("router", &fixture_binary),
            fixture_module("delivery", &fixture_binary),
        ],
        discovery: DiscoverySpec {
            namespace: "phase10".to_string(),
            modules: vec!["router".to_string(), "delivery".to_string()],
        },
        pre_spawn: vec![
            PreSpawnData {
                module_id: "router".to_string(),
                env: mcp_env(router_extra_env),
            },
            PreSpawnData {
                module_id: "delivery".to_string(),
                env: mcp_env(delivery_extra_env),
            },
        ],
    };

    start_meta_runtime(config, vec![], Duration::from_secs(2)).expect("runtime starts")
}

fn routing_delivery_runtime() -> meerkat_meta::MetaRuntimeHandle {
    routing_delivery_runtime_with_env(&[], &[])
}

fn parse_response(raw: &str) -> Value {
    serde_json::from_str(raw).expect("valid rpc response")
}

#[test]
fn phase0_contract_009_runtime_routes_and_delivery_history_project_without_host_invention() {
    let mut runtime = routing_delivery_runtime();

    let add_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-route-add","method":"meta/routing/routes/add","params":{"route":{"route_key":"vip-route","recipient":"vip@example.com","channel":"notification","sink":"sms","target_module":"delivery","retry_max":0,"backoff_ms":5,"rate_limit_per_minute":9}}}"#,
        Duration::from_secs(1),
    ));
    let listed_routes = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-route-list","method":"meta/routing/routes/list","params":{}}"#,
        Duration::from_secs(1),
    ));
    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-route-resolve","method":"meta/routing/resolve","params":{"recipient":"vip@example.com","channel":"notification"}}"#,
        Duration::from_secs(1),
    ));
    let sent = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase0-delivery-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase0-contract-009",
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-delivery-history","method":"meta/delivery/history","params":{"recipient":"vip@example.com","limit":10}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        add_route["result"]["route"]["route_key"],
        json!("vip-route")
    );
    assert_eq!(
        listed_routes["result"]["routes"],
        json!([
            {
                "route_key":"vip-route",
                "recipient":"vip@example.com",
                "channel":"notification",
                "sink":"sms",
                "target_module":"delivery",
                "retry_max":0,
                "backoff_ms":5,
                "rate_limit_per_minute":9,
            }
        ])
    );
    assert_eq!(sent["result"]["status"], json!("sent"));
    assert_eq!(
        history["result"]["deliveries"]
            .as_array()
            .map_or(0, Vec::len),
        1
    );
    assert_eq!(
        history["result"]["deliveries"][0]["recipient"],
        json!("vip@example.com")
    );
    assert_eq!(history["result"]["deliveries"][0]["sink"], json!("sms"));
    assert_eq!(
        history["result"]["deliveries"][0]["target_module"],
        json!("delivery")
    );
    assert_eq!(
        history["result"]["deliveries"][0]["attempts"],
        json!([
            {
                "attempt": 1,
                "status": "sent",
                "backoff_ms": 0,
            }
        ])
    );
}

#[test]
fn phase0_contract_009_delivery_send_survives_slow_mcp_subprocess() {
    let mut runtime = routing_delivery_runtime_with_env(
        &[],
        &[
            (DELAY_ON_ENV, "call_tool:delivery.send"),
            (DELAY_MS_ENV, "1500"),
        ],
    );

    let add_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-slow-route-add","method":"meta/routing/routes/add","params":{"route":{"route_key":"slow-route","recipient":"vip@example.com","channel":"notification","sink":"sms","target_module":"delivery","retry_max":0,"backoff_ms":5,"rate_limit_per_minute":9}}}"#,
        Duration::from_secs(1),
    ));
    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-slow-route-resolve","method":"meta/routing/resolve","params":{"recipient":"vip@example.com","channel":"notification"}}"#,
        Duration::from_secs(1),
    ));
    let sent = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase0-slow-delivery-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase0-contract-009-slow",
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase0-slow-delivery-history","method":"meta/delivery/history","params":{"recipient":"vip@example.com","limit":10}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(
        add_route["result"]["route"]["route_key"],
        json!("slow-route")
    );
    assert!(
        sent["error"].is_null(),
        "slow delivery should not time out under the core MCP budget: {sent}"
    );
    assert_eq!(sent["result"]["status"], json!("sent"));
    assert_eq!(
        history["result"]["deliveries"]
            .as_array()
            .map_or(0, Vec::len),
        1
    );
}

#[test]
#[ignore]
fn phase10_choke_107_routing_resolve_hands_off_to_delivery_send() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","channel":"transactional"}}"#,
        Duration::from_secs(1),
    ));
    let sent = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(resolved["result"]["target_module"], json!("delivery"));
    assert_eq!(resolved["result"]["sink"], json!("email"));
    assert_eq!(sent["result"]["route_id"], resolved["result"]["route_id"]);
    assert_eq!(sent["result"]["status"], json!("sent"));
    assert_eq!(
        sent["result"]["attempts"],
        json!([
            {"attempt":1,"status":"transient_failure","backoff_ms":250},
            {"attempt":2,"status":"sent","backoff_ms":0}
        ])
    );
}

#[test]
#[ignore]
fn phase10_e2e_1001_routing_delivery_flow_history_and_rate_limit() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-e2e-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com"}}"#,
        Duration::from_secs(1),
    ));

    let send_a = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-send-a",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-key-a",
                "payload": {"message": "a"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let send_b = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-send-b",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-key-b",
                "payload": {"message": "b"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let send_c = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-send-c",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-key-c",
                "payload": {"message": "c"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-history","method":"meta/delivery/history","params":{"recipient":"user@example.com","limit":10}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(send_a["result"]["status"], json!("sent"));
    assert_eq!(send_b["result"]["status"], json!("sent"));
    assert_eq!(send_c["error"]["code"], json!(-32602));
    assert_eq!(
        send_c["error"]["message"],
        json!("Invalid params: rate limit exceeded for sink 'email' at window 0 (limit=2)")
    );
    assert_eq!(
        history["result"]["deliveries"]
            .as_array()
            .map_or(0, Vec::len),
        2
    );
    assert_eq!(
        history["result"]["deliveries"][0]["route_id"],
        resolved["result"]["route_id"]
    );
    assert_eq!(
        history["result"]["deliveries"][1]["route_id"],
        resolved["result"]["route_id"]
    );
}

#[test]
#[ignore]
fn phase10_rpc_invalid_params_for_routing_and_delivery_are_typed() {
    let mut runtime = routing_delivery_runtime();

    let invalid_resolve = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-invalid-resolve","method":"meta/routing/resolve","params":{}}"#,
        Duration::from_secs(1),
    ));
    let invalid_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-invalid-send","method":"meta/delivery/send","params":{"payload":{"message":"x"}}}"#,
        Duration::from_secs(1),
    ));
    let invalid_send_idempotency = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-invalid-send-idempotency","method":"meta/delivery/send","params":{"resolution":{"route_id":"r-1","recipient":"user@example.com","channel":"notification","sink":"email","target_module":"delivery","retry_max":1,"backoff_ms":250,"rate_limit_per_minute":2},"idempotency_key":"","payload":{"message":"x"}}}"#,
        Duration::from_secs(1),
    ));
    let invalid_history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-invalid-history","method":"meta/delivery/history","params":{"limit":0}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(invalid_resolve["error"]["code"], json!(-32602));
    assert_eq!(
        invalid_resolve["error"]["message"],
        json!("Invalid params: recipient must be a non-empty string")
    );

    assert_eq!(invalid_send["error"]["code"], json!(-32602));
    assert_eq!(
        invalid_send["error"]["message"],
        json!("Invalid params: resolution must be an object")
    );

    assert_eq!(invalid_send_idempotency["error"]["code"], json!(-32602));
    assert_eq!(
        invalid_send_idempotency["error"]["message"],
        json!("Invalid params: idempotency_key must be a non-empty string")
    );

    assert_eq!(invalid_history["error"]["code"], json!(-32602));
    assert_eq!(
        invalid_history["error"]["message"],
        json!("Invalid params: limit must be an integer between 1 and 200")
    );
}

#[test]
#[ignore]
fn phase10_forged_delivery_send_resolution_is_rejected() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-forged-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","channel":"transactional"}}"#,
        Duration::from_secs(1),
    ));
    let mut forged_resolution = resolved["result"].clone();
    forged_resolution["sink"] = json!("sms");
    let forged_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-forged-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": forged_resolution,
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(forged_send["error"]["code"], json!(-32602));
    assert_eq!(
        forged_send["error"]["message"],
        json!("Invalid params: resolution does not match the trusted route for route_id")
    );
}

#[test]
#[ignore]
fn phase10_retry_max_greater_than_one_is_honored_deterministically() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-retry-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":3,"backoff_ms":125}}"#,
        Duration::from_secs(1),
    ));
    let sent = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-retry-send",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "payload": {"message": "hello"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(sent["result"]["status"], json!("sent"));
    assert_eq!(
        sent["result"]["attempts"],
        json!([
            {"attempt":1,"status":"transient_failure","backoff_ms":125},
            {"attempt":2,"status":"transient_failure","backoff_ms":125},
            {"attempt":3,"status":"transient_failure","backoff_ms":125},
            {"attempt":4,"status":"sent","backoff_ms":0}
        ])
    );
    assert_eq!(
        sent["result"]["final_attempt_ms"]
            .as_u64()
            .unwrap_or_default()
            - sent["result"]["first_attempt_ms"]
                .as_u64()
                .unwrap_or_default(),
        375
    );
}

#[test]
#[ignore]
fn phase10_idempotency_payload_mismatch_and_post_eviction_replay_are_correct() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-idem-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":1000}}"#,
        Duration::from_secs(1),
    ));
    let first_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-idem-send-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-shared-key",
                "payload": {"message": "hello-a"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let mismatch_replay = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-idem-send-mismatch",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-shared-key",
                "payload": {"message": "hello-b"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    for idx in 0..201 {
        let _ = parse_response(&handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("phase10-idem-fill-{idx}"),
                "method": "meta/delivery/send",
                "params": {
                    "resolution": resolved["result"].clone(),
                    "idempotency_key": format!("phase10-fill-key-{idx}"),
                    "payload": {"message": format!("fill-{idx}")}
                }
            })
            .to_string(),
            Duration::from_secs(1),
        ));
    }

    let replay_after_eviction = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-idem-send-after-evict",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "idempotency_key": "phase10-shared-key",
                "payload": {"message": "hello-a"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let history = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-idem-history","method":"meta/delivery/history","params":{"recipient":"user@example.com","limit":200}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(mismatch_replay["error"]["code"], json!(-32602));
    assert_eq!(
        mismatch_replay["error"]["message"],
        json!("Invalid params: idempotency_key replay payload does not match original request")
    );
    assert_eq!(replay_after_eviction["result"]["status"], json!("sent"));
    assert_ne!(
        replay_after_eviction["result"]["delivery_id"],
        first_send["result"]["delivery_id"]
    );
    assert_eq!(
        history["result"]["deliveries"]
            .as_array()
            .map_or(0, Vec::len),
        200
    );
}

#[test]
#[ignore]
fn phase10_retry_max_above_cap_is_rejected() {
    let mut runtime = routing_delivery_runtime();

    let rejected = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-retry-cap","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":11}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(rejected["error"]["code"], json!(-32602));
    assert_eq!(
        rejected["error"]["message"],
        json!("Invalid params: retry_max must be <= 10")
    );
}

#[test]
#[ignore]
fn phase10_retry_max_and_rate_limit_validation_values_are_rejected() {
    let mut runtime = routing_delivery_runtime();

    let retry_overflow = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-retry-overflow",
            "method": "meta/routing/resolve",
            "params": {
                "recipient": "user@example.com",
                "retry_max": 4_294_967_296_u64
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let rate_limit_overflow = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-rate-overflow",
            "method": "meta/routing/resolve",
            "params": {
                "recipient": "user@example.com",
                "rate_limit_per_minute": 4_294_967_296_u64
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let rate_limit_zero = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-rate-zero",
            "method": "meta/routing/resolve",
            "params": {
                "recipient": "user@example.com",
                "rate_limit_per_minute": 0
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(retry_overflow["error"]["code"], json!(-32602));
    assert_eq!(
        retry_overflow["error"]["message"],
        json!("Invalid params: retry_max exceeds maximum supported integer range")
    );
    assert_eq!(rate_limit_overflow["error"]["code"], json!(-32602));
    assert_eq!(
        rate_limit_overflow["error"]["message"],
        json!("Invalid params: rate_limit_per_minute exceeds maximum supported integer range")
    );
    assert_eq!(rate_limit_zero["error"]["code"], json!(-32602));
    assert_eq!(
        rate_limit_zero["error"]["message"],
        json!("Invalid params: rate_limit_per_minute must be greater than 0")
    );
}

#[test]
#[ignore]
fn phase10_rate_window_counts_remain_single_window_for_rapid_sends() {
    let mut runtime = routing_delivery_runtime();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-prune-resolve","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":1000}}"#,
        Duration::from_secs(1),
    ));

    for idx in 0..121 {
        let sent = parse_response(&handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("phase10-prune-send-{idx}"),
                "method": "meta/delivery/send",
                "params": {
                    "resolution": resolved["result"].clone(),
                    "idempotency_key": format!("phase10-prune-key-{idx}"),
                    "payload": {"message": format!("prune-{idx}")}
                }
            })
            .to_string(),
            Duration::from_secs(1),
        ));
        assert_eq!(sent["result"]["status"], json!("sent"));
    }

    assert_eq!(runtime.delivery_rate_window_count_entries(), 1);
    runtime.shutdown();
}

#[test]
#[ignore]
fn phase10_rate_limit_is_isolated_per_route_scope() {
    let mut runtime = routing_delivery_runtime();

    let route_a = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-a","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":2}}"#,
        Duration::from_secs(1),
    ));
    let send_a_1 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-a-send-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-route-a-key-1",
                "payload": {"message": "a1"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let send_a_2 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-a-send-2",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-route-a-key-2",
                "payload": {"message": "a2"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let send_a_3 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-a-send-3",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-route-a-key-3",
                "payload": {"message": "a3"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    let route_b = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-b","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":2}}"#,
        Duration::from_secs(1),
    ));
    let send_b_1 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-b-send-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_b["result"].clone(),
                "idempotency_key": "phase10-route-b-key-1",
                "payload": {"message": "b1"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(send_a_1["result"]["status"], json!("sent"));
    assert_eq!(send_a_2["result"]["status"], json!("sent"));
    assert_eq!(send_a_3["error"]["code"], json!(-32602));
    assert_eq!(
        send_a_3["error"]["message"],
        json!("Invalid params: rate limit exceeded for sink 'email' at window 0 (limit=2)")
    );
    assert_eq!(send_b_1["result"]["status"], json!("sent"));
}

#[test]
#[ignore]
fn phase10_rejected_rate_limited_calls_do_not_advance_delivery_clock() {
    let mut runtime = routing_delivery_runtime();

    let route_a = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-clock-rate-route-a","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":1}}"#,
        Duration::from_secs(1),
    ));
    let first_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-clock-rate-send-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-clock-rate-key-1",
                "payload": {"message": "first"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    for idx in 0..5 {
        let rejected = parse_response(&handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("phase10-clock-rate-reject-{idx}"),
                "method": "meta/delivery/send",
                "params": {
                    "resolution": route_a["result"].clone(),
                    "idempotency_key": format!("phase10-clock-rate-reject-key-{idx}"),
                    "payload": {"message": format!("reject-{idx}")}
                }
            })
            .to_string(),
            Duration::from_secs(1),
        ));
        assert_eq!(rejected["error"]["code"], json!(-32602));
        assert_eq!(
            rejected["error"]["message"],
            json!("Invalid params: rate limit exceeded for sink 'email' at window 0 (limit=1)")
        );
    }

    let route_b = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-clock-rate-route-b","method":"meta/routing/resolve","params":{"recipient":"other@example.com","retry_max":0,"rate_limit_per_minute":1000}}"#,
        Duration::from_secs(1),
    ));
    let second_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-clock-rate-send-2",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_b["result"].clone(),
                "idempotency_key": "phase10-clock-rate-key-2",
                "payload": {"message": "second"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(first_send["result"]["status"], json!("sent"));
    assert_eq!(second_send["result"]["status"], json!("sent"));
    assert_eq!(
        second_send["result"]["first_attempt_ms"]
            .as_u64()
            .unwrap_or_default(),
        first_send["result"]["first_attempt_ms"]
            .as_u64()
            .unwrap_or_default()
            .saturating_add(1_000)
    );
}

#[test]
#[ignore]
fn phase10_rate_limit_cannot_be_bypassed_via_artificial_delivery_clock_advancement() {
    let mut runtime = routing_delivery_runtime();

    let route_a = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-rate-now-route-a","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":1}}"#,
        Duration::from_secs(1),
    ));
    let send_a_1 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-rate-now-send-a-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-rate-now-key-a-1",
                "payload": {"message": "a1"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    let route_b = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-rate-now-route-b","method":"meta/routing/resolve","params":{"recipient":"other@example.com","retry_max":10,"backoff_ms":60000,"rate_limit_per_minute":1000}}"#,
        Duration::from_secs(1),
    ));
    let send_b_1 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-rate-now-send-b-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_b["result"].clone(),
                "idempotency_key": "phase10-rate-now-key-b-1",
                "payload": {"message": "b1"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    let send_a_2 = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-rate-now-send-a-2",
            "method": "meta/delivery/send",
            "params": {
                "resolution": route_a["result"].clone(),
                "idempotency_key": "phase10-rate-now-key-a-2",
                "payload": {"message": "a2"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(send_a_1["result"]["status"], json!("sent"));
    assert_eq!(send_b_1["result"]["status"], json!("sent"));
    assert_eq!(send_a_2["error"]["code"], json!(-32602));
    assert!(
        send_a_2["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("rate limit exceeded for sink 'email'"))
    );
}

#[test]
#[ignore]
fn phase10_routing_resolved_timestamp_never_regresses_merged_timeline() {
    let mut runtime = routing_delivery_runtime();
    let baseline_max_timestamp_ms = runtime
        .merged_events()
        .iter()
        .map(|envelope| envelope.timestamp_ms)
        .max()
        .unwrap_or_default();

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-resolved-ts-floor","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":2}}"#,
        Duration::from_secs(1),
    ));
    let route_id = resolved["result"]["route_id"]
        .as_str()
        .expect("resolved route id");
    let resolved_timestamp_ms = runtime
        .merged_events()
        .iter()
        .find_map(|envelope| match &envelope.event {
            UnifiedEvent::Module(event)
                if event.module == "router"
                    && event.event_type == "resolved"
                    && event
                        .payload
                        .get("route_id")
                        .and_then(Value::as_str)
                        .is_some_and(|candidate| candidate == route_id) =>
            {
                Some(envelope.timestamp_ms)
            }
            _ => None,
        })
        .expect("resolve event timestamp");

    runtime.shutdown();

    assert!(resolved_timestamp_ms >= baseline_max_timestamp_ms);
}

#[test]
#[ignore]
fn phase10_delivery_final_attempt_preserves_monotonic_event_timestamps() {
    let mut runtime = routing_delivery_runtime();

    let first_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-monotonic-resolve-1","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":3,"backoff_ms":125}}"#,
        Duration::from_secs(1),
    ));
    let first_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-monotonic-send-1",
            "method": "meta/delivery/send",
            "params": {
                "resolution": first_route["result"].clone(),
                "idempotency_key": "phase10-monotonic-key-1",
                "payload": {"message": "first"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let second_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-monotonic-resolve-2","method":"meta/routing/resolve","params":{"recipient":"another@example.com","retry_max":0}}"#,
        Duration::from_secs(1),
    ));

    let second_route_id = second_route["result"]["route_id"]
        .as_str()
        .expect("second route id");
    let second_resolve_ts = runtime
        .merged_events()
        .iter()
        .find_map(|envelope| match &envelope.event {
            UnifiedEvent::Module(event)
                if event.module == "router"
                    && event.event_type == "resolved"
                    && event
                        .payload
                        .get("route_id")
                        .and_then(Value::as_str)
                        .is_some_and(|route_id| route_id == second_route_id) =>
            {
                Some(envelope.timestamp_ms)
            }
            _ => None,
        })
        .expect("find router resolved event for second route");

    runtime.shutdown();

    assert_eq!(first_send["result"]["status"], json!("sent"));
    assert!(
        second_resolve_ts
            >= first_send["result"]["final_attempt_ms"]
                .as_u64()
                .unwrap_or_default()
    );
}

#[test]
#[ignore]
fn phase10_idempotent_replay_survives_routing_resolution_cache_eviction() {
    let mut runtime = routing_delivery_runtime();

    let original_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-evict-resolve-0","method":"meta/routing/resolve","params":{"recipient":"user@example.com","retry_max":0,"rate_limit_per_minute":1000}}"#,
        Duration::from_secs(1),
    ));
    let first_send = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-evict-send-0",
            "method": "meta/delivery/send",
            "params": {
                "resolution": original_route["result"].clone(),
                "idempotency_key": "phase10-route-evict-key",
                "payload": {"message": "same"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    for idx in 0..513 {
        let _ = parse_response(&handle_meta_rpc_json(
            &mut runtime,
            &json!({
                "jsonrpc": "2.0",
                "id": format!("phase10-route-evict-resolve-fill-{idx}"),
                "method": "meta/routing/resolve",
                "params": {
                    "recipient": format!("evict-{idx}@example.com"),
                    "retry_max": 0,
                    "rate_limit_per_minute": 1000
                }
            })
            .to_string(),
            Duration::from_secs(1),
        ));
    }
    let mut forged_resolution_after_eviction = original_route["result"].clone();
    forged_resolution_after_eviction["sink"] = json!("sms");

    let non_replay_after_eviction = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-evict-send-non-replay",
            "method": "meta/delivery/send",
            "params": {
                "resolution": original_route["result"].clone(),
                "idempotency_key": "phase10-route-evict-new-key",
                "payload": {"message": "new"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let forged_replay_after_eviction = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-evict-send-forged-replay",
            "method": "meta/delivery/send",
            "params": {
                "resolution": forged_resolution_after_eviction,
                "idempotency_key": "phase10-route-evict-key",
                "payload": {"message": "same"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let replay_after_eviction = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-route-evict-send-replay",
            "method": "meta/delivery/send",
            "params": {
                "resolution": original_route["result"].clone(),
                "idempotency_key": "phase10-route-evict-key",
                "payload": {"message": "same"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(non_replay_after_eviction["error"]["code"], json!(-32602));
    assert_eq!(
        non_replay_after_eviction["error"]["message"],
        json!(
            "Invalid params: resolution.route_id 'route-000000' was not issued by routing/resolve"
        )
    );
    assert_eq!(forged_replay_after_eviction["error"]["code"], json!(-32602));
    assert_eq!(
        forged_replay_after_eviction["error"]["message"],
        json!("Invalid params: resolution does not match the trusted route for route_id")
    );
    assert_eq!(
        replay_after_eviction["result"]["delivery_id"],
        first_send["result"]["delivery_id"]
    );
}

#[test]
#[ignore]
fn phase10_routing_and_delivery_paths_consume_module_boundary_outputs() {
    let mut runtime = routing_delivery_runtime_with_env(
        &[("META_PHASE_C_ROUTER_SINK", "webhook")],
        &[("META_PHASE_C_DELIVERY_ADAPTER", "smtp-mock")],
    );

    let resolved = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-boundary-resolve","method":"meta/routing/resolve","params":{"recipient":"boundary@example.com","channel":"transactional"}}"#,
        Duration::from_secs(1),
    ));
    let sent_ok = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-boundary-send-ok",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "payload": {"message": "ok"}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));
    let sent_failed = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        &json!({
            "jsonrpc": "2.0",
            "id": "phase10-boundary-send-fail",
            "method": "meta/delivery/send",
            "params": {
                "resolution": resolved["result"].clone(),
                "payload": {"message": "bad", "force_adapter_fail": true}
            }
        })
        .to_string(),
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(resolved["result"]["sink"], json!("webhook"));
    assert_eq!(sent_ok["result"]["status"], json!("sent"));
    assert_eq!(sent_ok["result"]["sink_adapter"], json!("smtp-mock"));
    assert_eq!(sent_failed["result"]["status"], json!("failed"));
    assert_eq!(
        sent_failed["result"]["attempts"][1]["status"],
        json!("failed")
    );
}

#[test]
#[ignore]
fn phase10_runtime_route_update_surface_changes_resolve_behavior() {
    let mut runtime = routing_delivery_runtime();

    let list_initial = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-list-initial","method":"meta/routing/routes/list","params":{}}"#,
        Duration::from_secs(1),
    ));
    let add_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-add","method":"meta/routing/routes/add","params":{"route":{"route_key":"vip-route","recipient":"vip@example.com","channel":"notification","sink":"sms","target_module":"delivery","retry_max":0,"backoff_ms":5,"rate_limit_per_minute":9}}}"#,
        Duration::from_secs(1),
    ));
    let list_after_add = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-list-after-add","method":"meta/routing/routes/list","params":{}}"#,
        Duration::from_secs(1),
    ));
    let resolved_with_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-resolve-with-route","method":"meta/routing/resolve","params":{"recipient":"vip@example.com","channel":"notification"}}"#,
        Duration::from_secs(1),
    ));
    let deleted_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-delete","method":"meta/routing/routes/delete","params":{"route_key":"vip-route"}}"#,
        Duration::from_secs(1),
    ));
    let resolved_without_route = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-resolve-without-route","method":"meta/routing/resolve","params":{"recipient":"vip@example.com","channel":"notification"}}"#,
        Duration::from_secs(1),
    ));
    let invalid_add = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"phase10-route-add-invalid","method":"meta/routing/routes/add","params":{"route":{"recipient":"oops@example.com","sink":"email","target_module":"delivery"}}}"#,
        Duration::from_secs(1),
    ));

    runtime.shutdown();

    assert_eq!(list_initial["result"]["routes"], json!([]));
    assert_eq!(
        add_route["result"]["route"]["route_key"],
        json!("vip-route")
    );
    assert_eq!(
        list_after_add["result"]["routes"]
            .as_array()
            .map_or(0, Vec::len),
        1
    );
    assert_eq!(resolved_with_route["result"]["sink"], json!("sms"));
    assert_eq!(resolved_with_route["result"]["retry_max"], json!(0));
    assert_eq!(resolved_with_route["result"]["backoff_ms"], json!(5));
    assert_eq!(
        resolved_with_route["result"]["rate_limit_per_minute"],
        json!(9)
    );
    assert_eq!(
        deleted_route["result"]["deleted"]["route_key"],
        json!("vip-route")
    );
    assert_eq!(resolved_without_route["result"]["sink"], json!("email"));
    assert_eq!(invalid_add["error"]["code"], json!(-32602));
    assert_eq!(
        invalid_add["error"]["message"],
        json!("Invalid params: route_key must be a non-empty string")
    );
}
