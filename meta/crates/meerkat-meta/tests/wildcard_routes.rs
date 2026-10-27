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

use meerkat_meta::WILDCARD_ROUTE;
use meerkat_meta::{
    DiscoverySpec, MetaConfig, ModuleConfig, PreSpawnData, RestartPolicy, handle_meta_rpc_json,
    start_meta_runtime,
};
use serde_json::{Value, json};

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

fn mcp_env() -> Vec<(String, String)> {
    vec![(
        BOUNDARY_ENV_KEY.to_string(),
        BOUNDARY_ENV_VALUE_MCP.to_string(),
    )]
}

fn wildcard_test_runtime() -> meerkat_meta::MetaRuntimeHandle {
    let fixture_binary = fixture_binary_path();
    let config = MetaConfig {
        modules: vec![
            fixture_module("router", &fixture_binary),
            fixture_module("delivery", &fixture_binary),
        ],
        discovery: DiscoverySpec {
            namespace: "mk019".to_string(),
            modules: vec!["router".to_string(), "delivery".to_string()],
        },
        pre_spawn: vec![
            PreSpawnData {
                module_id: "router".to_string(),
                env: mcp_env(),
            },
            PreSpawnData {
                module_id: "delivery".to_string(),
                env: mcp_env(),
            },
        ],
    };

    start_meta_runtime(config, vec![], Duration::from_secs(2)).expect("runtime starts")
}

fn parse_response(raw: &str) -> Value {
    serde_json::from_str(raw).expect("valid rpc response")
}

fn add_route(runtime: &mut meerkat_meta::MetaRuntimeHandle, id: &str, route_json: &str) -> Value {
    let rpc = format!(
        r#"{{"jsonrpc":"2.0","id":"{}","method":"meta/routing/routes/add","params":{{"route":{}}}}}"#,
        id, route_json
    );
    parse_response(&handle_meta_rpc_json(runtime, &rpc, Duration::from_secs(1)))
}

fn resolve_route(
    runtime: &mut meerkat_meta::MetaRuntimeHandle,
    id: &str,
    recipient: &str,
    channel: &str,
) -> Value {
    let rpc = format!(
        r#"{{"jsonrpc":"2.0","id":"{}","method":"meta/routing/resolve","params":{{"recipient":"{}","channel":"{}"}}}}"#,
        id, recipient, channel
    );
    parse_response(&handle_meta_rpc_json(runtime, &rpc, Duration::from_secs(1)))
}

/// Regression: exact match still works without wildcards.
#[test]
#[ignore]
fn mk019_exact_match_regression() {
    let mut runtime = wildcard_test_runtime();

    add_route(
        &mut runtime,
        "mk019-exact-add",
        r#"{"route_key":"exact-route","recipient":"alice@example.com","channel":"notification","sink":"sms","target_module":"delivery","retry_max":2,"backoff_ms":100,"rate_limit_per_minute":5}"#,
    );

    let resolved = resolve_route(
        &mut runtime,
        "mk019-exact-resolve",
        "alice@example.com",
        "notification",
    );
    runtime.shutdown();

    assert_eq!(resolved["result"]["sink"], json!("sms"));
    assert_eq!(resolved["result"]["retry_max"], json!(2));
}

/// Wildcard recipient "*" matches any recipient.
#[test]
#[ignore]
fn mk019_wildcard_recipient_matches_any() {
    let mut runtime = wildcard_test_runtime();

    add_route(
        &mut runtime,
        "mk019-wild-recip-add",
        r#"{"route_key":"catch-all-recip","recipient":"*","channel":"alert","sink":"webhook","target_module":"delivery","retry_max":1,"backoff_ms":50,"rate_limit_per_minute":3}"#,
    );

    let resolved = resolve_route(
        &mut runtime,
        "mk019-wild-recip-resolve",
        "anyone@example.com",
        "alert",
    );
    runtime.shutdown();

    assert_eq!(resolved["result"]["sink"], json!("webhook"));
    assert_eq!(resolved["result"]["retry_max"], json!(1));
}

/// Wildcard channel "*" matches any channel.
#[test]
#[ignore]
fn mk019_wildcard_channel_matches_any() {
    let mut runtime = wildcard_test_runtime();

    add_route(
        &mut runtime,
        "mk019-wild-chan-add",
        r#"{"route_key":"catch-all-chan","recipient":"bob@example.com","channel":"*","sink":"sms","target_module":"delivery","retry_max":3,"backoff_ms":200,"rate_limit_per_minute":7}"#,
    );

    let resolved = resolve_route(
        &mut runtime,
        "mk019-wild-chan-resolve",
        "bob@example.com",
        "transactional",
    );
    runtime.shutdown();

    assert_eq!(resolved["result"]["sink"], json!("sms"));
    assert_eq!(resolved["result"]["retry_max"], json!(3));
}

/// Double wildcard (recipient="*", channel="*") matches everything.
#[test]
#[ignore]
fn mk019_double_wildcard_matches_everything() {
    let mut runtime = wildcard_test_runtime();

    add_route(
        &mut runtime,
        "mk019-double-wild-add",
        r#"{"route_key":"global-catch-all","recipient":"*","channel":"*","sink":"webhook","target_module":"delivery","retry_max":1,"backoff_ms":10,"rate_limit_per_minute":2}"#,
    );

    let resolved = resolve_route(
        &mut runtime,
        "mk019-double-wild-resolve",
        "unknown@example.com",
        "marketing",
    );
    runtime.shutdown();

    assert_eq!(resolved["result"]["sink"], json!("webhook"));
    assert_eq!(resolved["result"]["retry_max"], json!(1));
}

/// Exact match takes priority over wildcard.
#[test]
#[ignore]
fn mk019_exact_match_takes_priority_over_wildcard() {
    let mut runtime = wildcard_test_runtime();

    // Add wildcard catch-all first
    add_route(
        &mut runtime,
        "mk019-priority-wild-add",
        r#"{"route_key":"global-catch-all","recipient":"*","channel":"*","sink":"webhook","target_module":"delivery","retry_max":1,"backoff_ms":10,"rate_limit_per_minute":2}"#,
    );
    // Add exact route second
    add_route(
        &mut runtime,
        "mk019-priority-exact-add",
        r#"{"route_key":"vip-exact","recipient":"vip@example.com","channel":"notification","sink":"sms","target_module":"delivery","retry_max":5,"backoff_ms":300,"rate_limit_per_minute":10}"#,
    );

    // The exact match should win over the wildcard
    let resolved = resolve_route(
        &mut runtime,
        "mk019-priority-resolve",
        "vip@example.com",
        "notification",
    );
    runtime.shutdown();

    assert_eq!(resolved["result"]["sink"], json!("sms"));
    assert_eq!(resolved["result"]["retry_max"], json!(5));
    assert_eq!(resolved["result"]["backoff_ms"], json!(300));
    assert_eq!(resolved["result"]["rate_limit_per_minute"], json!(10));
}

/// Exact recipient + wildcard channel has priority over wildcard recipient + exact channel.
#[test]
#[ignore]
fn mk019_exact_recipient_wildcard_channel_priority() {
    let mut runtime = wildcard_test_runtime();

    // Priority 4: double wildcard
    add_route(
        &mut runtime,
        "mk019-p4-add",
        r#"{"route_key":"p4-double-wild","recipient":"*","channel":"*","sink":"webhook","target_module":"delivery","retry_max":1,"backoff_ms":10,"rate_limit_per_minute":2}"#,
    );
    // Priority 3: wildcard recipient + exact channel
    add_route(
        &mut runtime,
        "mk019-p3-add",
        r#"{"route_key":"p3-wild-recip","recipient":"*","channel":"alert","sink":"sms","target_module":"delivery","retry_max":2,"backoff_ms":20,"rate_limit_per_minute":3}"#,
    );
    // Priority 2: exact recipient + wildcard channel
    add_route(
        &mut runtime,
        "mk019-p2-add",
        r#"{"route_key":"p2-wild-chan","recipient":"carol@example.com","channel":"*","sink":"email","target_module":"delivery","retry_max":3,"backoff_ms":30,"rate_limit_per_minute":4}"#,
    );

    // carol@example.com + alert should match p2 (exact recipient + wildcard channel) over p3 (wildcard recipient + exact channel)
    let resolved = resolve_route(
        &mut runtime,
        "mk019-priority-p2-resolve",
        "carol@example.com",
        "alert",
    );
    runtime.shutdown();

    assert_eq!(resolved["result"]["sink"], json!("email"));
    assert_eq!(resolved["result"]["retry_max"], json!(3));
}

/// Wildcard recipient + exact channel beats double wildcard.
#[test]
#[ignore]
fn mk019_wildcard_recipient_exact_channel_beats_double_wildcard() {
    let mut runtime = wildcard_test_runtime();

    // Double wildcard
    add_route(
        &mut runtime,
        "mk019-dw-add",
        r#"{"route_key":"double-wild","recipient":"*","channel":"*","sink":"webhook","target_module":"delivery","retry_max":1,"backoff_ms":10,"rate_limit_per_minute":2}"#,
    );
    // Wildcard recipient + exact channel
    add_route(
        &mut runtime,
        "mk019-wrec-add",
        r#"{"route_key":"wild-recip-exact-chan","recipient":"*","channel":"urgent","sink":"sms","target_module":"delivery","retry_max":4,"backoff_ms":40,"rate_limit_per_minute":6}"#,
    );

    let resolved = resolve_route(
        &mut runtime,
        "mk019-wrec-resolve",
        "anyone@example.com",
        "urgent",
    );
    runtime.shutdown();

    assert_eq!(resolved["result"]["sink"], json!("sms"));
    assert_eq!(resolved["result"]["retry_max"], json!(4));
}

/// add_runtime_route accepts "*" as a valid recipient (it passes the non-empty check).
#[test]
#[ignore]
fn mk019_add_route_accepts_wildcard_recipient() {
    let mut runtime = wildcard_test_runtime();

    let result = add_route(
        &mut runtime,
        "mk019-add-wild-valid",
        r#"{"route_key":"wild-route","recipient":"*","channel":"notification","sink":"webhook","target_module":"delivery","retry_max":1,"backoff_ms":10,"rate_limit_per_minute":2}"#,
    );
    runtime.shutdown();

    assert_eq!(result["result"]["route"]["recipient"], json!("*"));
    assert_eq!(result["result"]["route"]["route_key"], json!("wild-route"));
}

/// The WILDCARD_ROUTE constant equals "*".
#[test]
#[ignore]
fn mk019_wildcard_constant_value() {
    assert_eq!(WILDCARD_ROUTE, "*");
}
