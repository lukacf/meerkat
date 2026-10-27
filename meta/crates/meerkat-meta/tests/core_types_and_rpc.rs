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
    ConfigResolutionError, DiscoverySpec, EventEnvelope, MetaConfig, MockModuleProcess,
    MockProcessError, ModuleConfig, ModuleEvent, NormalizationError, PreSpawnData,
    ProtocolParseError, RestartPolicy, RpcCapabilitiesError, RuntimeFromConfigError, UnifiedEvent,
    normalize_event_line, parse_module_event_line, parse_rpc_capabilities,
    parse_unified_event_line, run_discovered_module_once,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReadyPayload {
    version: String,
}

fn sample_module_line() -> String {
    json!({
        "event_id": "evt-1",
        "source": "module",
        "timestamp_ms": 42,
        "event": {
            "kind": "module",
            "module": "echo",
            "event_type": "ready",
            "payload": {
                "version": "1.0.0"
            }
        }
    })
    .to_string()
}

#[test]
fn type_001_event_envelope_and_events_round_trip() {
    let envelope = EventEnvelope {
        event_id: "evt-agent".to_string(),
        source: "agent".to_string(),
        timestamp_ms: 7,
        event: UnifiedEvent::Agent {
            agent_id: "agent-1".to_string(),
            event_type: "heartbeat".to_string(),
            payload: Some(json!({"status": "ok"})),
        },
    };

    let serialized = serde_json::to_string(&envelope).expect("serialize envelope");
    let decoded: EventEnvelope<UnifiedEvent> =
        serde_json::from_str(&serialized).expect("deserialize envelope");

    assert_eq!(decoded, envelope);

    let module_event = ModuleEvent {
        module: "mod-a".to_string(),
        event_type: "ready".to_string(),
        payload: json!({"ok": true}),
    };
    let serialized = serde_json::to_string(&module_event).expect("serialize module event");
    let decoded: ModuleEvent = serde_json::from_str(&serialized).expect("deserialize module event");
    assert_eq!(decoded, module_event);
}

#[test]
fn type_002_module_config_and_restart_policy_round_trip() {
    let config = ModuleConfig {
        id: "echo".to_string(),
        command: "echo-module".to_string(),
        args: vec!["--json-line".to_string()],
        restart_policy: RestartPolicy::OnFailure,
    };

    let serialized = serde_json::to_string(&config).expect("serialize module config");
    let decoded: ModuleConfig =
        serde_json::from_str(&serialized).expect("deserialize module config");

    assert_eq!(decoded, config);
}

#[test]
fn type_003_bootstrap_and_discovery_round_trip() {
    let config = MetaConfig {
        modules: vec![ModuleConfig {
            id: "echo".to_string(),
            command: "echo-module".to_string(),
            args: vec!["--json-line".to_string()],
            restart_policy: RestartPolicy::Never,
        }],
        discovery: DiscoverySpec {
            namespace: "default".to_string(),
            modules: vec!["echo".to_string()],
        },
        pre_spawn: vec![PreSpawnData {
            module_id: "echo".to_string(),
            env: vec![("RUST_LOG".to_string(), "debug".to_string())],
        }],
    };

    let serialized = serde_json::to_string(&config).expect("serialize meta config");
    let decoded: MetaConfig = serde_json::from_str(&serialized).expect("deserialize meta config");

    assert_eq!(decoded, config);
}

#[test]
fn contract_001_valid_event_line_parses() {
    let line = sample_module_line();

    let decoded = parse_unified_event_line(&line).expect("parse unified line");
    assert_eq!(decoded.event_id, "evt-1");
    assert_eq!(decoded.source, "module");
}

#[test]
fn contract_001_schema_invalid_response_rejected() {
    let invalid_line = json!({
        "source": "module",
        "timestamp_ms": 42,
        "event": {"kind": "module"}
    })
    .to_string();

    let err = parse_unified_event_line(&invalid_line).expect_err("invalid schema should fail");
    assert_eq!(err, ProtocolParseError::InvalidSchema);
}

#[test]
fn contract_001_unexpected_type_payload_rejected() {
    let wrong_payload = json!({
        "event_id": "evt-2",
        "source": "module",
        "timestamp_ms": 42,
        "event": {
            "kind": "module",
            "module": "echo",
            "event_type": "ready",
            "payload": {"unexpected": true}
        }
    })
    .to_string();

    let err = parse_module_event_line::<ReadyPayload>(&wrong_payload, "ready")
        .expect_err("payload type mismatch should fail");
    assert_eq!(err, ProtocolParseError::UnexpectedPayloadType);
}

#[test]
fn contract_002_malformed_event_lines_rejected_with_typed_errors() {
    let invalid_json = normalize_event_line("{").expect_err("invalid json should fail");
    assert_eq!(invalid_json, NormalizationError::InvalidJson);

    let invalid_schema = normalize_event_line("[]").expect_err("invalid schema should fail");
    assert_eq!(invalid_schema, NormalizationError::InvalidSchema);

    let missing_field = normalize_event_line(
        &json!({
            "event_id": "evt-missing",
            "timestamp_ms": 10,
            "agent_id": "a-1",
            "event_type": "tick"
        })
        .to_string(),
    )
    .expect_err("missing source should fail");
    assert_eq!(missing_field, NormalizationError::MissingField("source"));
}

#[test]
fn choke_001_mixed_agent_and_module_lines_normalize_through_shared_runtime_path() {
    let agent_line = json!({
        "event_id": "evt-agent",
        "source": "agent",
        "timestamp_ms": 1,
        "agent_id": "a-1",
        "event_type": "tick"
    })
    .to_string();
    let module_line = json!({
        "event_id": "evt-module",
        "source": "module",
        "timestamp_ms": 2,
        "module": "m-1",
        "event_type": "ready",
        "payload": {"version": "1"}
    })
    .to_string();

    let stream = vec![
        normalize_event_line(&agent_line).expect("agent normalize"),
        normalize_event_line(&module_line).expect("module normalize"),
    ];
    assert_eq!(stream[0].event_id, "evt-agent");
    assert_eq!(stream[0].source, "agent");
    assert_eq!(stream[0].timestamp_ms, 1);
    assert_eq!(
        stream[0].event,
        UnifiedEvent::Agent {
            agent_id: "a-1".to_string(),
            event_type: "tick".to_string(),
            payload: None,
        }
    );

    assert_eq!(stream[1].event_id, "evt-module");
    assert_eq!(stream[1].source, "module");
    assert_eq!(stream[1].timestamp_ms, 2);
    assert_eq!(
        stream[1].event,
        UnifiedEvent::Module(ModuleEvent {
            module: "m-1".to_string(),
            event_type: "ready".to_string(),
            payload: json!({"version": "1"}),
        })
    );
}

#[test]
fn contract_005_rpc_capabilities_requires_contract_version() {
    let missing = parse_rpc_capabilities("{}").expect_err("missing key should fail");
    assert_eq!(missing, RpcCapabilitiesError::MissingContractVersion);

    let invalid = parse_rpc_capabilities(r#"{"contract_version":42}"#)
        .expect_err("invalid contract version type");
    assert_eq!(invalid, RpcCapabilitiesError::InvalidContractVersion);

    let parsed = parse_rpc_capabilities(r#"{"contract_version":"0.1","foo":"bar"}"#)
        .expect("parse capabilities");
    assert_eq!(parsed.contract_version, "0.1");
}

#[test]
fn type_003_runtime_config_resolution_is_typed() {
    let config = MetaConfig {
        modules: vec![],
        discovery: DiscoverySpec {
            namespace: "default".to_string(),
            modules: vec![],
        },
        pre_spawn: vec![],
    };

    let err = run_discovered_module_once(&config, "echo", Duration::from_millis(10))
        .expect_err("missing module should fail");
    assert_eq!(
        err,
        RuntimeFromConfigError::Config(ConfigResolutionError::ModuleNotConfigured(
            "echo".to_string()
        ))
    );
}

#[test]
fn adversarial_fail_then_succeed_mock_behavior() {
    let mock = MockModuleProcess::fail_then_succeed(1);
    let ok_line = sample_module_line();

    let first = mock.invoke_json_line_with_timeout(Duration::from_millis(2), &ok_line);
    let second = mock.invoke_json_line_with_timeout(Duration::from_millis(2), &ok_line);

    assert_eq!(first, Err(MockProcessError::LaunchFailed));
    assert!(second.is_ok());
    assert_eq!(mock.attempts(), 2);
}

#[test]
fn adversarial_never_responds_mock_behavior_timeout_driven() {
    let mock = MockModuleProcess::never_responds();
    let result = mock.invoke_json_line_with_timeout(Duration::from_millis(1), "{}");
    assert_eq!(result, Err(MockProcessError::Timeout));
}
