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
use std::time::Duration;

use meerkat_meta::{
    DiscoverySpec, META_CONTRACT_VERSION, MetaConfig, ModuleConfig, PreSpawnData, RestartPolicy,
    handle_meta_rpc_json, start_meta_runtime,
};
use serde_json::{Value, json};

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

#[test]
fn rpc_001_builtins_status_capabilities_events_reconcile_and_spawn_member() {
    let config = MetaConfig {
        modules: vec![
            shell_module(
                "router",
                r#"printf '%s\n' '{"event_id":"evt-router","source":"module","timestamp_ms":10,"event":{"kind":"module","module":"router","event_type":"ready","payload":{"ok":true}}}'"#,
            ),
            shell_module(
                "delivery",
                r#"printf '%s\n' '{"event_id":"evt-delivery","source":"module","timestamp_ms":20,"event":{"kind":"module","module":"delivery","event_type":"ready","payload":{"ok":true}}}'"#,
            ),
        ],
        discovery: DiscoverySpec {
            namespace: "phase4".to_string(),
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

    let mut runtime =
        start_meta_runtime(config, vec![], Duration::from_secs(1)).expect("runtime starts");

    let status = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"1","method":"meta/status","params":{}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(status["result"]["contract_version"], META_CONTRACT_VERSION);
    assert_eq!(status["result"]["running"], true);
    assert_eq!(status["result"]["loaded_modules"], json!(["router"]));

    let caps = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"2","method":"meta/capabilities","params":{}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(caps["result"]["contract_version"], META_CONTRACT_VERSION);
    assert!(
        caps["result"]["methods"]
            .as_array()
            .expect("methods array")
            .iter()
            .any(|method| method == "meta/reconcile")
    );

    let events = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"3","method":"meta/events/subscribe","params":{}}"#,
        Duration::from_secs(1),
    ));
    assert!(
        events["result"]["events"]
            .as_array()
            .expect("events array")
            .iter()
            .any(|event| event["event_id"] == "evt-router")
    );

    let reconcile = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"4","method":"meta/reconcile","params":{"modules":["router","delivery"]}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(reconcile["result"]["accepted"], true);
    assert_eq!(reconcile["result"]["added"], 1);

    let spawn_member = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"5","method":"meta/spawn_member","params":{"module_id":"delivery"}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(spawn_member["result"]["accepted"], true);
    assert_eq!(spawn_member["result"]["module_id"], "delivery");

    let events_after_spawn = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"6","method":"meta/events/subscribe","params":{}}"#,
        Duration::from_secs(1),
    ));
    assert!(
        events_after_spawn["result"]["events"]
            .as_array()
            .expect("events array")
            .iter()
            .any(|event| event["event_id"] == "evt-delivery")
    );
}

#[test]
fn rpc_002_module_proxy_and_unloaded_module_error_shape() {
    let config = MetaConfig {
        modules: vec![shell_module(
            "analytics",
            r#"printf '%s\n' '{"event_id":"evt-analytics","source":"module","timestamp_ms":42,"event":{"kind":"module","module":"analytics","event_type":"response","payload":{"via":"analytics"}}}'"#,
        )],
        discovery: DiscoverySpec {
            namespace: "phase4".to_string(),
            modules: vec!["analytics".to_string()],
        },
        pre_spawn: vec![PreSpawnData {
            module_id: "analytics".to_string(),
            env: vec![],
        }],
    };

    let mut runtime =
        start_meta_runtime(config, vec![], Duration::from_secs(1)).expect("runtime starts");

    let routed = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"10","method":"analytics/echo","params":{"msg":"ok"}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(routed["result"]["module_id"], "analytics");
    assert_eq!(routed["result"]["method"], "analytics/echo");
    assert_eq!(routed["result"]["payload"], json!({"via":"analytics"}));

    let unloaded = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"11","method":"gating/pending","params":{}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(
        unloaded,
        json!({
            "jsonrpc":"2.0",
            "id":"11",
            "error":{"code":-32601,"message":"Module 'gating' not loaded"}
        })
    );
}

#[test]
fn rpc_001_notifications_apply_runtime_mutations_without_response() {
    let config = MetaConfig {
        modules: vec![
            shell_module(
                "router",
                r#"printf '%s\n' '{"event_id":"evt-router","source":"module","timestamp_ms":10,"event":{"kind":"module","module":"router","event_type":"ready","payload":{"ok":true}}}'"#,
            ),
            shell_module(
                "delivery",
                r#"printf '%s\n' '{"event_id":"evt-delivery","source":"module","timestamp_ms":20,"event":{"kind":"module","module":"delivery","event_type":"ready","payload":{"ok":true}}}'"#,
            ),
        ],
        discovery: DiscoverySpec {
            namespace: "phase4".to_string(),
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

    let mut runtime =
        start_meta_runtime(config, vec![], Duration::from_secs(1)).expect("runtime starts");

    let notification_response = handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","method":"meta/spawn_member","params":{"module_id":"delivery"}}"#,
        Duration::from_secs(1),
    );
    assert_eq!(notification_response, "");

    let status = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"12","method":"meta/status","params":{}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(
        status["result"]["loaded_modules"],
        json!(["delivery", "router"])
    );
}

#[test]
fn rpc_001_invalid_requests_return_jsonrpc_errors() {
    let config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "phase4".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };
    let mut runtime =
        start_meta_runtime(config, vec![], Duration::from_millis(200)).expect("runtime starts");

    let parse_err = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        "{",
        Duration::from_secs(1),
    ));
    assert_eq!(parse_err["error"]["code"], -32700);

    let bad_jsonrpc = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"1.0","id":"x","method":"meta/status","params":{}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(bad_jsonrpc["error"]["code"], -32600);

    let non_object_request = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"["jsonrpc","2.0"]"#,
        Duration::from_secs(1),
    ));
    assert_eq!(non_object_request["error"]["code"], -32600);

    let missing_method = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"missing-method","params":{}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(missing_method["error"]["code"], -32600);

    let invalid_method_type = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"invalid-method","method":123,"params":{}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(invalid_method_type["error"]["code"], -32600);

    let method_not_found = parse_response(&handle_meta_rpc_json(
        &mut runtime,
        r#"{"jsonrpc":"2.0","id":"x","method":"meta/unknown","params":{}}"#,
        Duration::from_secs(1),
    ));
    assert_eq!(method_not_found["error"]["code"], -32601);
}
