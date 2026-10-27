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
use std::{fs, process::Command, time::Duration};

use meerkat_meta::{
    BaselineRuntimeError, DiscoverySpec, MEERKAT_REPO_ENV, MetaConfig, ModuleConfig,
    NormalizationError, PreSpawnData, ProcessBoundaryError, ProtocolParseError,
    REQUIRED_MEERKAT_SYMBOLS, RestartPolicy, RpcRuntimeError, RuntimeBoundaryError, UnifiedEvent,
    parse_module_event_line, run_discovered_module_once, run_meerkat_baseline_verification_once,
    run_module_boundary_once, run_rpc_capabilities_boundary_once,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ReadyPayload {
    version: String,
}

fn shell_module(id: &str, script: &str) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), script.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

#[test]
#[ignore = "external boundary subprocess check"]
fn external_valid_json_line_from_subprocess_parses() {
    let config = MetaConfig {
        modules: vec![shell_module(
            "echo",
            r#"printf '%s\n' '{"event_id":"evt-1","source":"module","timestamp_ms":42,"event":{"kind":"module","module":"echo","event_type":"ready","payload":{"version":"1.0.0"}}}'"#,
        )],
        discovery: DiscoverySpec {
            namespace: "default".to_string(),
            modules: vec!["echo".to_string()],
        },
        pre_spawn: vec![PreSpawnData {
            module_id: "echo".to_string(),
            env: vec![("RUST_LOG".to_string(), "debug".to_string())],
        }],
    };

    let envelope = run_discovered_module_once(&config, "echo", Duration::from_secs(1))
        .expect("valid process output should parse");
    assert_eq!(envelope.event_id, "evt-1");
    assert_eq!(envelope.source, "module");
    assert_eq!(envelope.timestamp_ms, 42);
    assert_eq!(
        envelope.event,
        UnifiedEvent::Module(meerkat_meta::ModuleEvent {
            module: "echo".to_string(),
            event_type: "ready".to_string(),
            payload: json!({"version":"1.0.0"}),
        })
    );
}

#[test]
#[ignore = "external boundary subprocess check"]
fn external_invalid_schema_from_subprocess_rejected() {
    let module = shell_module(
        "bad",
        r#"printf '%s\n' '{"source":"module","timestamp_ms":42,"event":{"kind":"module"}}'"#,
    );

    let err = run_module_boundary_once(&module, None, Duration::from_secs(1))
        .expect_err("schema must reject");
    assert_eq!(
        err,
        RuntimeBoundaryError::Normalize(NormalizationError::MissingField("event_id"))
    );
}

#[test]
#[ignore = "external boundary subprocess check"]
fn external_timeout_on_never_responding_subprocess() {
    let module = shell_module("sleepy", "sleep 5");
    let err = run_module_boundary_once(&module, None, Duration::from_millis(25))
        .expect_err("must timeout");
    assert_eq!(
        err,
        RuntimeBoundaryError::Process(ProcessBoundaryError::Timeout { timeout_ms: 25 })
    );
}

#[test]
#[ignore = "external boundary subprocess check"]
fn external_normalization_path_over_mixed_subprocess_outputs() {
    let agent = shell_module(
        "agent-line",
        r#"printf '%s\n' '{"event_id":"evt-agent","source":"agent","timestamp_ms":1,"agent_id":"a-1","event_type":"tick"}'"#,
    );
    let module = shell_module(
        "module-line",
        r#"printf '%s\n' '{"event_id":"evt-module","source":"module","timestamp_ms":2,"module":"mod-1","event_type":"ready","payload":{"ok":true}}'"#,
    );

    let first = run_module_boundary_once(&agent, None, Duration::from_secs(1)).expect("agent line");
    let second =
        run_module_boundary_once(&module, None, Duration::from_secs(1)).expect("module line");

    assert_eq!(first.event_id, "evt-agent");
    assert_eq!(first.source, "agent");
    assert_eq!(first.timestamp_ms, 1);
    assert_eq!(
        first.event,
        UnifiedEvent::Agent {
            agent_id: "a-1".to_string(),
            event_type: "tick".to_string(),
            payload: None,
        }
    );

    assert_eq!(second.event_id, "evt-module");
    assert_eq!(second.source, "module");
    assert_eq!(second.timestamp_ms, 2);
    assert_eq!(
        second.event,
        UnifiedEvent::Module(meerkat_meta::ModuleEvent {
            module: "mod-1".to_string(),
            event_type: "ready".to_string(),
            payload: json!({"ok":true}),
        })
    );
}

#[test]
#[ignore = "external boundary subprocess check"]
fn external_rpc_capabilities_requires_contract_version_from_process_response() {
    let caps = run_rpc_capabilities_boundary_once(
        "sh",
        &[
            "-c".to_string(),
            r#"printf '%s\n' '{"contract_version":"0.1.0","transport":"stdio"}'"#.to_string(),
        ],
        &[],
        Duration::from_secs(1),
    )
    .expect("process should return capabilities");
    assert_eq!(caps.contract_version, "0.1.0");

    let missing = run_rpc_capabilities_boundary_once(
        "sh",
        &[
            "-c".to_string(),
            r#"printf '%s\n' '{"transport":"stdio"}'"#.to_string(),
        ],
        &[],
        Duration::from_secs(1),
    )
    .expect_err("missing contract_version must fail");
    assert_eq!(
        missing,
        RpcRuntimeError::Capabilities(meerkat_meta::RpcCapabilitiesError::MissingContractVersion)
    );
}

#[test]
#[ignore = "external boundary subprocess check"]
fn external_meerkat_baseline_symbols_check_against_repo_path() {
    let repo_root = std::env::var(MEERKAT_REPO_ENV)
        .expect("set MEERKAT_REPO to run the external Meerkat baseline check");
    let payload = json!({ "repo_root": repo_root }).to_string();
    let report = run_meerkat_baseline_verification_once(
        "sh",
        &[
            "-c".to_string(),
            "printf '%s\\n' \"$BASELINE_REPO_PAYLOAD\"".to_string(),
        ],
        &[("BASELINE_REPO_PAYLOAD".to_string(), payload)],
        Duration::from_secs(1),
    )
    .expect("baseline symbols should exist");
    assert!(report.missing_symbols.is_empty());
}

#[test]
#[ignore = "external boundary subprocess check"]
fn external_meerkat_baseline_missing_symbols_has_typed_diagnostics() {
    let temp = tempfile::tempdir().expect("temp dir");
    let err = run_meerkat_baseline_verification_once(
        "sh",
        &[
            "-c".to_string(),
            format!(
                "printf '%s\\n' '{{\"repo_root\":\"{}\"}}'",
                temp.path().display()
            ),
        ],
        &[],
        Duration::from_secs(1),
    )
    .expect_err("empty repo should fail");
    assert!(matches!(err, BaselineRuntimeError::Baseline(_)));
}

#[test]
#[ignore = "external boundary subprocess check"]
fn external_unexpected_payload_from_subprocess_is_rejected() {
    let line = run_module_boundary_once(
        &shell_module(
            "bad-payload",
            r#"printf '%s\n' '{"event_id":"evt-x","source":"module","timestamp_ms":42,"event":{"kind":"module","module":"echo","event_type":"ready","payload":{"unexpected":true}}}'"#,
        ),
        None,
        Duration::from_secs(1),
    )
    .expect("line should normalize");

    let envelope = serde_json::to_string(&line).expect("serialize normalized envelope");
    let err = parse_module_event_line::<ReadyPayload>(&envelope, "ready")
        .expect_err("typed payload mismatch should fail");
    assert_eq!(err, ProtocolParseError::UnexpectedPayloadType);
}

#[test]
fn phase0_baseline_check_binary_requires_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_baseline_check"))
        .env_remove(MEERKAT_REPO_ENV)
        .output()
        .expect("phase0 baseline binary should run");

    assert!(
        !output.status.success(),
        "unconfigured phase0_baseline_check unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not configured"));
    assert!(stderr.contains("MEERKAT_REPO"));
}

#[test]
fn phase0_baseline_check_binary_uses_configured_repo() {
    let repo = tempfile::tempdir().expect("temporary Meerkat repository");
    fs::write(
        repo.path().join("baseline-symbols.rs"),
        REQUIRED_MEERKAT_SYMBOLS.join("\n"),
    )
    .expect("write baseline symbols");

    let output = Command::new(env!("CARGO_BIN_EXE_baseline_check"))
        .env(MEERKAT_REPO_ENV, repo.path())
        .output()
        .expect("phase0 baseline binary should run");

    assert!(
        output.status.success(),
        "configured phase0_baseline_check failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&format!("repo={}", repo.path().display())));
    assert!(stdout.contains("missing_symbols="));
}
