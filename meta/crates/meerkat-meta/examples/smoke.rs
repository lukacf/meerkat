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
    DiscoverySpec, EventEnvelope, MetaConfig, ModuleConfig, ModuleRouteRequest, RestartPolicy,
    UnifiedEvent, route_module_call, route_module_call_rpc_json, route_module_call_rpc_subprocess,
    start_meta_runtime,
};
use serde_json::json;

fn module(id: &str, script: &str) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn main() {
    let config = MetaConfig {
        modules: vec![
            module(
                "messenger",
                r#"printf '%s\n' '{"event_id":"evt-messenger-ready","source":"module","timestamp_ms":20,"event":{"kind":"module","module":"messenger","event_type":"ready","payload":{"ok":true,"status":"online"}}}'"#,
            ),
            module(
                "notifier",
                r#"printf '%s\n' '{"event_id":"evt-notifier-ready","source":"module","timestamp_ms":30,"event":{"kind":"module","module":"notifier","event_type":"ready","payload":{"ok":true,"status":"online"}}}'"#,
            ),
        ],
        discovery: DiscoverySpec {
            namespace: "smoke".to_string(),
            modules: vec!["messenger".to_string(), "notifier".to_string()],
        },
        pre_spawn: vec![],
    };

    let agent_events = vec![
        EventEnvelope {
            event_id: "evt-agent-1".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 10,
            event: UnifiedEvent::Agent {
                agent_id: "user-bridge".to_string(),
                event_type: "message_received".to_string(),
                payload: None,
            },
        },
        EventEnvelope {
            event_id: "evt-agent-2".to_string(),
            source: "agent".to_string(),
            timestamp_ms: 40,
            event: UnifiedEvent::Agent {
                agent_id: "user-bridge".to_string(),
                event_type: "message_sent".to_string(),
                payload: None,
            },
        },
    ];

    let mut runtime =
        start_meta_runtime(config, agent_events, Duration::from_secs(2)).expect("start runtime");
    println!("SMOKE: runtime running={}", runtime.is_running());
    println!("SMOKE: loaded modules={:?}", runtime.loaded_modules());

    println!("SMOKE: merged stream");
    for event in runtime.merged_events() {
        println!(
            "  - id={} src={} ts={} kind={:?}",
            event.event_id, event.source, event.timestamp_ms, event.event
        );
    }

    // Library-mode call
    let req = ModuleRouteRequest {
        module_id: "messenger".to_string(),
        method: "send_message".to_string(),
        params: json!({
            "to":"demo-user",
            "text":"hello from smoke test"
        }),
    };
    let library_resp = route_module_call(&runtime, &req, Duration::from_secs(2))
        .expect("library route call succeeds");
    println!("SMOKE: library route response={:?}", library_resp);

    // RPC-JSON call (same behavior through JSON boundary)
    let req_json = serde_json::to_string(&req).expect("serialize request");
    let rpc_json_resp = route_module_call_rpc_json(&runtime, &req_json, Duration::from_secs(2))
        .expect("rpc json route call succeeds");
    println!("SMOKE: rpc-json route response={}", rpc_json_resp);

    // RPC-subprocess call (request arrives from a subprocess boundary)
    let emit_request = vec![
        "-c".to_string(),
        format!("printf '%s\\n' '{}'", req_json.replace('\'', "'\"'\"'")),
    ];
    let rpc_subprocess_resp = route_module_call_rpc_subprocess(
        &runtime,
        "sh",
        &emit_request,
        &[],
        Duration::from_secs(2),
    )
    .expect("rpc subprocess route call succeeds");
    println!(
        "SMOKE: rpc-subprocess route response={}",
        rpc_subprocess_resp
    );

    let shutdown = runtime.shutdown();
    println!("SMOKE: shutdown={:?}", shutdown);
    println!("SMOKE: runtime running={}", runtime.is_running());
}
