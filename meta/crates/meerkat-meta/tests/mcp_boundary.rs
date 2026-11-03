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
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use meerkat_meta::runtime::{
    DeliverySendError, DeliverySendRequest, RoutingResolveError, RoutingResolveRequest,
};
use meerkat_meta::{
    DiscoverySpec, GatingEvaluateRequest, GatingOutcome, GatingRiskTier, McpBoundaryError,
    MetaConfig, ModuleConfig, ModuleRouteError, ModuleRouteRequest, RestartPolicy,
    RuntimeBoundaryError, start_meta_runtime,
};
use serde_json::json;
use tempfile::tempdir;

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

fn fixture_module(id: &str, fixture_binary: &Path) -> ModuleConfig {
    ModuleConfig {
        id: id.to_string(),
        command: fixture_binary.display().to_string(),
        args: vec!["--module".to_string(), id.to_string()],
        restart_policy: RestartPolicy::Never,
    }
}

fn legacy_ready_module(id: &str) -> ModuleConfig {
    let ready_event = format!(
        r#"{{"event_id":"evt-{id}-legacy","source":"module","timestamp_ms":1,"event":{{"kind":"module","module":"{id}","event_type":"ready","payload":{{"legacy":true}}}}}}"#
    );
    ModuleConfig {
        id: id.to_string(),
        command: "sh".to_string(),
        args: vec!["-c".to_string(), format!("printf '%s\\n' '{ready_event}'")],
        restart_policy: RestartPolicy::Never,
    }
}

fn mcp_env(log_path: &Path, extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut env = vec![
        (
            BOUNDARY_ENV_KEY.to_string(),
            BOUNDARY_ENV_VALUE_MCP.to_string(),
        ),
        (
            "META_PHASE_C_LOG_PATH".to_string(),
            log_path.display().to_string(),
        ),
    ];
    env.extend(
        extra
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string())),
    );
    env
}

fn mcp_env_with_hang_control(
    log_path: &Path,
    hang_control_path: &Path,
    extra: &[(&str, &str)],
) -> Vec<(String, String)> {
    let mut env = mcp_env(log_path, extra);
    env.push((
        "META_PHASE_C_HANG_ON_FILE".to_string(),
        hang_control_path.display().to_string(),
    ));
    env
}

fn set_hang_targets(path: &Path, targets: &str) {
    fs::write(path, targets).expect("write hang control file");
}

fn log_lines(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(path)
        .expect("read fixture log")
        .lines()
        .map(ToString::to_string)
        .collect()
}

#[test]
#[ignore]
fn phase_c_req_c001_c002_mcp_path_is_used_for_configured_router_and_delivery_modules() {
    let fixture_binary = fixture_binary_path();
    let temp = tempdir().expect("tempdir");
    let log_path = temp.path().join("phase-c-routing-delivery.log");

    let mut runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![
                fixture_module("router", &fixture_binary),
                fixture_module("delivery", &fixture_binary),
            ],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec!["router".to_string(), "delivery".to_string()],
            },
            pre_spawn: vec![
                meerkat_meta::PreSpawnData {
                    module_id: "router".to_string(),
                    env: mcp_env(&log_path, &[("META_PHASE_C_ROUTER_SINK", "mcp-email")]),
                },
                meerkat_meta::PreSpawnData {
                    module_id: "delivery".to_string(),
                    env: mcp_env(
                        &log_path,
                        &[("META_PHASE_C_DELIVERY_ADAPTER", "mcp-adapter")],
                    ),
                },
            ],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("runtime with mcp modules should start");

    let resolution = runtime
        .resolve_routing(RoutingResolveRequest {
            recipient: "approvals@example.com".to_string(),
            channel: Some("transactional".to_string()),
            retry_max: None,
            backoff_ms: None,
            rate_limit_per_minute: None,
        })
        .expect("routing should succeed through MCP path");
    let delivery = runtime
        .send_delivery(DeliverySendRequest {
            resolution: resolution.clone(),
            payload: json!({"kind":"phase-c"}),
            idempotency_key: Some("phase-c-routing-delivery".to_string()),
        })
        .expect("delivery should succeed through MCP path");
    runtime.shutdown();

    assert_eq!(resolution.sink, "mcp-email");
    assert_eq!(resolution.target_module, "delivery");
    assert_eq!(delivery.sink_adapter.as_deref(), Some("mcp-adapter"));

    let lines = log_lines(&log_path);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("router:call:routing.resolve")),
        "expected router MCP tool call in log"
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains("delivery:call:delivery.send")),
        "expected delivery MCP tool call in log"
    );
}

#[test]
#[ignore]
fn phase_c_req_c003_c004_c006_core_orchestrates_gating_router_then_delivery() {
    let fixture_binary = fixture_binary_path();
    let temp = tempdir().expect("tempdir");
    let log_path = temp.path().join("phase-c-orchestration.log");

    let mut runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![
                fixture_module("router", &fixture_binary),
                fixture_module("delivery", &fixture_binary),
            ],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec!["router".to_string(), "delivery".to_string()],
            },
            pre_spawn: vec![
                meerkat_meta::PreSpawnData {
                    module_id: "router".to_string(),
                    env: mcp_env(&log_path, &[]),
                },
                meerkat_meta::PreSpawnData {
                    module_id: "delivery".to_string(),
                    env: mcp_env(&log_path, &[]),
                },
            ],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("runtime with mcp modules should start");

    let evaluated = runtime.evaluate_gating_action(GatingEvaluateRequest {
        action: "publish_release".to_string(),
        actor_id: "alice".to_string(),
        risk_tier: GatingRiskTier::R3,
        rationale: None,
        requested_approver: Some("bob".to_string()),
        approval_recipient: Some("approvals@example.com".to_string()),
        approval_channel: Some("transactional".to_string()),
        approval_timeout_ms: Some(60_000),
        entity: None,
        topic: None,
    });
    let pending = runtime.list_gating_pending();
    runtime.shutdown();

    assert_eq!(evaluated.outcome, GatingOutcome::PendingApproval);
    assert!(evaluated.pending_id.is_some());
    assert_eq!(pending.len(), 1);
    assert!(pending[0].approval_route_id.is_some());
    assert!(pending[0].approval_delivery_id.is_some());

    let calls = log_lines(&log_path)
        .into_iter()
        .filter(|line| line.contains(":call:"))
        .collect::<Vec<_>>();
    let router_idx = calls
        .iter()
        .position(|line| line.contains("router:call:routing.resolve"))
        .expect("router call should be present");
    let delivery_idx = calls
        .iter()
        .position(|line| line.contains("delivery:call:delivery.send"))
        .expect("delivery call should be present");
    assert!(router_idx < delivery_idx, "core must route before delivery");
}

#[test]
#[ignore]
fn phase_c_typed_errors_surface_for_unloaded_and_failed_mcp_modules() {
    let mut unloaded_runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec![],
            },
            pre_spawn: vec![],
        },
        vec![],
        Duration::from_secs(1),
    )
    .expect("empty runtime should start");
    let unloaded_route_error = unloaded_runtime.resolve_routing(RoutingResolveRequest {
        recipient: "approvals@example.com".to_string(),
        channel: Some("transactional".to_string()),
        retry_max: None,
        backoff_ms: None,
        rate_limit_per_minute: None,
    });
    let unloaded_module_error = meerkat_meta::route_module_call(
        &unloaded_runtime,
        &ModuleRouteRequest {
            module_id: "missing".to_string(),
            method: "missing/echo".to_string(),
            params: json!({}),
        },
        Duration::from_secs(1),
    );
    unloaded_runtime.shutdown();

    assert_eq!(
        unloaded_route_error,
        Err(RoutingResolveError::RouterModuleNotLoaded)
    );
    assert_eq!(
        unloaded_module_error,
        Err(ModuleRouteError::UnloadedModule("missing".to_string()))
    );

    let fixture_binary = fixture_binary_path();
    let temp = tempdir().expect("tempdir");
    let log_path = temp.path().join("phase-c-error.log");
    let mut failed_runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![
                fixture_module("router", &fixture_binary),
                fixture_module("delivery", &fixture_binary),
            ],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec!["router".to_string(), "delivery".to_string()],
            },
            pre_spawn: vec![
                meerkat_meta::PreSpawnData {
                    module_id: "router".to_string(),
                    env: mcp_env(&log_path, &[("META_PHASE_C_FAIL_TOOL", "routing.resolve")]),
                },
                meerkat_meta::PreSpawnData {
                    module_id: "delivery".to_string(),
                    env: mcp_env(&log_path, &[]),
                },
            ],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("runtime should start even when router tool call is configured to fail");

    let failed_route_error = failed_runtime.resolve_routing(RoutingResolveRequest {
        recipient: "approvals@example.com".to_string(),
        channel: Some("transactional".to_string()),
        retry_max: None,
        backoff_ms: None,
        rate_limit_per_minute: None,
    });
    let evaluated = failed_runtime.evaluate_gating_action(GatingEvaluateRequest {
        action: "publish_release".to_string(),
        actor_id: "alice".to_string(),
        risk_tier: GatingRiskTier::R3,
        rationale: None,
        requested_approver: Some("bob".to_string()),
        approval_recipient: Some("approvals@example.com".to_string()),
        approval_channel: Some("transactional".to_string()),
        approval_timeout_ms: Some(60_000),
        entity: None,
        topic: None,
    });
    let pending = failed_runtime.list_gating_pending();
    failed_runtime.shutdown();

    match failed_route_error {
        Err(RoutingResolveError::RouterBoundary(RuntimeBoundaryError::Mcp(
            McpBoundaryError::ToolCallFailed {
                module_id, tool, ..
            },
        ))) => {
            assert_eq!(module_id, "router");
            assert_eq!(tool, "routing.resolve");
        }
        other => panic!("expected typed MCP tool-call failure, got: {other:?}"),
    }

    assert_eq!(evaluated.outcome, GatingOutcome::PendingApproval);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].approval_route_id, None);
    assert_eq!(pending[0].approval_delivery_id, None);
    let lines = log_lines(&log_path);
    assert!(
        lines
            .iter()
            .any(|line| line.contains("router:call:routing.resolve")),
        "router call should be present"
    );
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("delivery:call:delivery.send")),
        "delivery must not be called when routing fails at core boundary"
    );
}

#[test]
#[ignore]
fn phase_c_req_c005_memory_conflict_read_uses_mcp() {
    let fixture_binary = fixture_binary_path();
    let temp = tempdir().expect("tempdir");
    let memory_log = temp.path().join("phase-c-memory.log");

    let mut memory_runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![fixture_module("memory", &fixture_binary)],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec!["memory".to_string()],
            },
            pre_spawn: vec![meerkat_meta::PreSpawnData {
                module_id: "memory".to_string(),
                env: mcp_env(
                    &memory_log,
                    &[
                        ("META_PHASE_C_MEMORY_CONFLICT_KEY", "router:deploy"),
                        ("META_PHASE_C_MEMORY_CONFLICT_REASON", "phase_c_conflict"),
                    ],
                ),
            }],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("memory runtime should start");

    let conflict_evaluated = memory_runtime.evaluate_gating_action(GatingEvaluateRequest {
        action: "deploy_prod".to_string(),
        actor_id: "alice".to_string(),
        risk_tier: GatingRiskTier::R2,
        rationale: None,
        requested_approver: None,
        approval_recipient: None,
        approval_channel: None,
        approval_timeout_ms: None,
        entity: Some("router".to_string()),
        topic: Some("deploy".to_string()),
    });
    memory_runtime.shutdown();

    assert_eq!(conflict_evaluated.outcome, GatingOutcome::SafeDraft);
    assert_eq!(
        conflict_evaluated.fallback_reason.as_deref(),
        Some("memory_conflict")
    );
    let memory_lines = log_lines(&memory_log);
    assert!(
        memory_lines
            .iter()
            .any(|line| line.contains("memory:call:memory.conflict_read")),
        "memory MCP conflict-read tool should be called"
    );
}

#[test]
#[ignore]
fn phase_c_req_c002_core_flows_require_mcp_for_loaded_core_modules() {
    let mut runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![
                legacy_ready_module("router"),
                legacy_ready_module("delivery"),
                legacy_ready_module("memory"),
            ],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec![
                    "router".to_string(),
                    "delivery".to_string(),
                    "memory".to_string(),
                ],
            },
            pre_spawn: vec![],
        },
        vec![],
        Duration::from_secs(1),
    )
    .expect("runtime with legacy modules should start");

    let routing_error = runtime.resolve_routing(RoutingResolveRequest {
        recipient: "approvals@example.com".to_string(),
        channel: Some("transactional".to_string()),
        retry_max: None,
        backoff_ms: None,
        rate_limit_per_minute: None,
    });
    let memory_evaluated = runtime.evaluate_gating_action(GatingEvaluateRequest {
        action: "deploy_prod".to_string(),
        actor_id: "alice".to_string(),
        risk_tier: GatingRiskTier::R2,
        rationale: None,
        requested_approver: None,
        approval_recipient: None,
        approval_channel: None,
        approval_timeout_ms: None,
        entity: Some("router".to_string()),
        topic: Some("deploy".to_string()),
    });
    let memory_audit = runtime.gating_audit_entries(8);
    runtime.shutdown();

    match routing_error {
        Err(RoutingResolveError::RouterBoundary(RuntimeBoundaryError::Mcp(
            McpBoundaryError::McpRequired { module_id, flow },
        ))) => {
            assert_eq!(module_id, "router");
            assert_eq!(flow, "routing.resolve");
        }
        other => panic!("expected MCP-required router error, got: {other:?}"),
    }

    assert_eq!(memory_evaluated.outcome, GatingOutcome::SafeDraft);
    assert_eq!(
        memory_evaluated.fallback_reason.as_deref(),
        Some("memory_conflict_lookup_failed")
    );
    assert!(
        memory_audit
            .iter()
            .any(|entry| entry.event_type == "memory_conflict_lookup_failed"),
        "expected memory_conflict_lookup_failed audit entry"
    );
}

#[test]
#[ignore]
fn phase_c_req_c002_generic_route_call_requires_mcp_for_loaded_core_modules() {
    let mut runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![
                legacy_ready_module("router"),
                legacy_ready_module("delivery"),
                legacy_ready_module("memory"),
            ],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec![
                    "router".to_string(),
                    "delivery".to_string(),
                    "memory".to_string(),
                ],
            },
            pre_spawn: vec![],
        },
        vec![],
        Duration::from_secs(1),
    )
    .expect("runtime with legacy core modules should start");

    for (module_id, method) in [
        ("router", "router.echo"),
        ("delivery", "delivery.echo"),
        ("memory", "memory.echo"),
    ] {
        let observed = meerkat_meta::route_module_call(
            &runtime,
            &ModuleRouteRequest {
                module_id: module_id.to_string(),
                method: method.to_string(),
                params: json!({}),
            },
            Duration::from_secs(1),
        );
        match observed {
            Err(ModuleRouteError::ModuleRuntime(RuntimeBoundaryError::Mcp(
                McpBoundaryError::McpRequired {
                    module_id: observed_module_id,
                    flow,
                },
            ))) => {
                assert_eq!(observed_module_id, module_id);
                assert_eq!(flow, method);
            }
            other => {
                panic!("expected generic route call to require MCP for {module_id}, got: {other:?}")
            }
        }
    }

    runtime.shutdown();
}

#[test]
#[ignore]
fn phase_c_req_c002_delivery_flow_requires_mcp_when_loaded() {
    let fixture_binary = fixture_binary_path();
    let temp = tempdir().expect("tempdir");
    let log_path = temp.path().join("phase-c-delivery-required.log");

    let mut runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![
                fixture_module("router", &fixture_binary),
                legacy_ready_module("delivery"),
            ],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec!["router".to_string(), "delivery".to_string()],
            },
            pre_spawn: vec![meerkat_meta::PreSpawnData {
                module_id: "router".to_string(),
                env: mcp_env(&log_path, &[]),
            }],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("runtime with mixed router/delivery should start");

    let resolution = runtime
        .resolve_routing(RoutingResolveRequest {
            recipient: "approvals@example.com".to_string(),
            channel: Some("transactional".to_string()),
            retry_max: None,
            backoff_ms: None,
            rate_limit_per_minute: None,
        })
        .expect("router resolve should still work through MCP");
    let delivery_error = runtime.send_delivery(DeliverySendRequest {
        resolution,
        payload: json!({"kind":"phase-c-delivery-required"}),
        idempotency_key: Some("phase-c-delivery-required".to_string()),
    });
    runtime.shutdown();

    match delivery_error {
        Err(DeliverySendError::DeliveryBoundary(RuntimeBoundaryError::Mcp(
            McpBoundaryError::McpRequired { module_id, flow },
        ))) => {
            assert_eq!(module_id, "delivery");
            assert_eq!(flow, "delivery.send");
        }
        other => panic!("expected MCP-required delivery error, got: {other:?}"),
    }
}

#[test]
#[ignore]
fn phase_c_req_c005_memory_mcp_failure_falls_back_to_safe_draft() {
    let fixture_binary = fixture_binary_path();
    let temp = tempdir().expect("tempdir");
    let log_path = temp.path().join("phase-c-memory-failure.log");

    let mut runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![fixture_module("memory", &fixture_binary)],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec!["memory".to_string()],
            },
            pre_spawn: vec![meerkat_meta::PreSpawnData {
                module_id: "memory".to_string(),
                env: mcp_env(
                    &log_path,
                    &[("META_PHASE_C_FAIL_TOOL", "memory.conflict_read")],
                ),
            }],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("memory runtime should start");

    let evaluated = runtime.evaluate_gating_action(GatingEvaluateRequest {
        action: "deploy_prod".to_string(),
        actor_id: "alice".to_string(),
        risk_tier: GatingRiskTier::R2,
        rationale: None,
        requested_approver: None,
        approval_recipient: None,
        approval_channel: None,
        approval_timeout_ms: None,
        entity: Some("router".to_string()),
        topic: Some("deploy".to_string()),
    });
    let audit = runtime.gating_audit_entries(8);
    runtime.shutdown();

    assert_eq!(evaluated.outcome, GatingOutcome::SafeDraft);
    assert_eq!(
        evaluated.fallback_reason.as_deref(),
        Some("memory_conflict_lookup_failed")
    );
    assert!(
        audit
            .iter()
            .any(|entry| entry.event_type == "memory_conflict_lookup_failed"),
        "expected memory_conflict_lookup_failed audit event"
    );
}

#[test]
#[ignore]
fn phase_c_cq001_connect_timeout_maps_to_typed_mcp_timeout() {
    let fixture_binary = fixture_binary_path();
    let temp = tempdir().expect("tempdir");
    let log_path = temp.path().join("phase-c-connect-timeout.log");
    let hang_control_path = temp.path().join("phase-c-hang-control-connect.txt");
    set_hang_targets(&hang_control_path, "");

    let mut runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![fixture_module("router", &fixture_binary)],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec!["router".to_string()],
            },
            pre_spawn: vec![meerkat_meta::PreSpawnData {
                module_id: "router".to_string(),
                env: mcp_env_with_hang_control(&log_path, &hang_control_path, &[]),
            }],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("runtime should start");

    set_hang_targets(&hang_control_path, "initialize");
    let route_error = meerkat_meta::route_module_call(
        &runtime,
        &ModuleRouteRequest {
            module_id: "router".to_string(),
            method: "routing.resolve".to_string(),
            params: json!({"recipient":"approvals@example.com"}),
        },
        Duration::from_secs(1),
    );
    runtime.shutdown();

    match route_error {
        Err(ModuleRouteError::ModuleRuntime(RuntimeBoundaryError::Mcp(
            McpBoundaryError::Timeout {
                module_id,
                operation,
                timeout_ms,
            },
        ))) => {
            assert_eq!(module_id, "router");
            assert_eq!(operation, "connect");
            assert_eq!(timeout_ms, 1_000);
        }
        other => panic!("expected typed timeout error, got: {other:?}"),
    }
}

#[test]
#[ignore]
fn phase_c_cq001_list_tools_timeout_maps_to_typed_timeout() {
    let fixture_binary = fixture_binary_path();
    let temp = tempdir().expect("tempdir");
    let log_path = temp.path().join("phase-c-list-tools-timeout.log");
    let hang_control_path = temp.path().join("phase-c-hang-control-list-tools.txt");
    set_hang_targets(&hang_control_path, "");

    let mut runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![fixture_module("router", &fixture_binary)],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec!["router".to_string()],
            },
            pre_spawn: vec![meerkat_meta::PreSpawnData {
                module_id: "router".to_string(),
                env: mcp_env_with_hang_control(&log_path, &hang_control_path, &[]),
            }],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("runtime should start");

    set_hang_targets(&hang_control_path, "list_tools");
    let route_error = meerkat_meta::route_module_call(
        &runtime,
        &ModuleRouteRequest {
            module_id: "router".to_string(),
            method: "routing.resolve".to_string(),
            params: json!({"recipient":"approvals@example.com"}),
        },
        Duration::from_secs(1),
    );
    runtime.shutdown();

    // The close is not bounded by the call's timeout: Meerkat bounds it, and
    // the hung fixture is ended by its escalation, so the close settles and
    // the timeout stands alone.
    match route_error {
        Err(ModuleRouteError::ModuleRuntime(RuntimeBoundaryError::Mcp(
            McpBoundaryError::Timeout {
                module_id,
                operation,
                timeout_ms,
            },
        ))) => {
            assert_eq!(module_id, "router");
            assert_eq!(operation, "list_tools");
            assert_eq!(timeout_ms, 1_000);
        }
        other => panic!("expected typed list_tools timeout error, got: {other:?}"),
    }
}

/// A slow close no longer fails the call: Meerkat's bounded close settles
/// it, and the primary tool failure stands alone. An unsettled close beside a
/// primary failure is covered in `runtime::module_boundary`.
#[test]
#[ignore]
fn phase_c_cq001_slow_close_settles_and_the_primary_error_stands_alone() {
    let fixture_binary = fixture_binary_path();
    let temp = tempdir().expect("tempdir");
    let log_path = temp.path().join("phase-c-close-failure.log");
    let hang_control_path = temp.path().join("phase-c-hang-control-close.txt");
    set_hang_targets(&hang_control_path, "");

    let mut runtime = start_meta_runtime(
        MetaConfig {
            modules: vec![fixture_module("router", &fixture_binary)],
            discovery: DiscoverySpec {
                namespace: "phase-c".to_string(),
                modules: vec!["router".to_string()],
            },
            pre_spawn: vec![meerkat_meta::PreSpawnData {
                module_id: "router".to_string(),
                env: mcp_env_with_hang_control(
                    &log_path,
                    &hang_control_path,
                    &[
                        ("META_PHASE_C_FAIL_TOOL", "routing.resolve"),
                        ("META_PHASE_C_CLOSE_DELAY_MS", "5000"),
                    ],
                ),
            }],
        },
        vec![],
        Duration::from_secs(2),
    )
    .expect("runtime should start");

    set_hang_targets(&hang_control_path, "close");
    let route_error = meerkat_meta::route_module_call(
        &runtime,
        &ModuleRouteRequest {
            module_id: "router".to_string(),
            method: "routing.resolve".to_string(),
            params: json!({"recipient":"approvals@example.com"}),
        },
        Duration::from_secs(1),
    );
    runtime.shutdown();

    match route_error {
        Err(ModuleRouteError::ModuleRuntime(RuntimeBoundaryError::Mcp(
            McpBoundaryError::ToolCallFailed {
                module_id,
                tool,
                reason: _,
            },
        ))) => {
            assert_eq!(module_id, "router");
            assert_eq!(tool, "routing.resolve");
        }
        other => panic!("expected the primary tool-call failure alone, got: {other:?}"),
    }
}
