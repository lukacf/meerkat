use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use meerkat_core::AgentToolDispatcher;
use meerkat_core::McpServerConfig;
use meerkat_core::event::ToolConfigChangeOperation;
use meerkat_mcp::{McpLifecycleAction, McpLifecyclePhase, McpRouter, McpRouterAdapter};

#[allow(clippy::expect_used, clippy::unwrap_used)]
fn test_server_path() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("workspace root")
        .join("target")
        .join("debug")
        .join("mcp-test-server")
}

fn skip_if_no_test_server() -> Option<PathBuf> {
    let path = test_server_path();
    if path.exists() {
        Some(path)
    } else {
        eprintln!(
            "Skipping lifecycle contract: mcp-test-server not built. Run `cargo build -p mcp-test-server` first."
        );
        None
    }
}

fn test_server_config(name: &str, path: &Path) -> McpServerConfig {
    McpServerConfig::stdio(
        name,
        path.to_string_lossy().to_string(),
        vec![],
        HashMap::new(),
    )
}

fn invalid_server_config(name: &str) -> McpServerConfig {
    McpServerConfig::stdio(
        name,
        "/definitely/not/a/real/mcp-server".to_string(),
        vec![],
        HashMap::new(),
    )
}

fn has_action(
    actions: &[McpLifecycleAction],
    target: &str,
    operation: ToolConfigChangeOperation,
    phase: McpLifecyclePhase,
) -> bool {
    let expected_persisted = !matches!(
        phase,
        McpLifecyclePhase::Pending | McpLifecyclePhase::Draining
    );
    actions.iter().any(|action| {
        action.target == target
            && action.operation == operation
            && action.phase == phase
            && action.persisted == expected_persisted
            && action.applied_at_turn.is_none()
    })
}

#[allow(clippy::expect_used, clippy::unwrap_used)]
async fn wait_for_adapter_action(
    adapter: &McpRouterAdapter,
    target: &str,
    operation: ToolConfigChangeOperation,
    phase: McpLifecyclePhase,
    timeout: Duration,
) -> Vec<McpLifecycleAction> {
    let deadline = Instant::now() + timeout;
    loop {
        let actions = adapter
            .poll_lifecycle_actions()
            .await
            .expect("poll lifecycle actions");
        if has_action(&actions, target, operation.clone(), phase) {
            return actions;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for lifecycle action {operation:?}/{phase:?} on {target}; last actions: {actions:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[allow(clippy::expect_used, clippy::unwrap_used)]
async fn wait_for_router_action(
    router: &mut McpRouter,
    target: &str,
    operation: ToolConfigChangeOperation,
    phase: McpLifecyclePhase,
    timeout: Duration,
) -> Vec<McpLifecycleAction> {
    let deadline = Instant::now() + timeout;
    loop {
        let actions = router.take_lifecycle_actions();
        if has_action(&actions, target, operation.clone(), phase) {
            return actions;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for lifecycle action {operation:?}/{phase:?} on {target}; last actions: {actions:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
#[ignore = "integration-real: requires mcp-test-server binary and real process spawning"]
#[allow(clippy::expect_used, clippy::unwrap_used)]
async fn lifecycle_contract_add_and_reload_emit_pending_then_applied_actions() {
    let Some(server_path) = skip_if_no_test_server() else {
        return;
    };

    let adapter = McpRouterAdapter::new(McpRouter::new());

    adapter
        .stage_add(test_server_config("lifecycle-add", &server_path))
        .await
        .expect("stage add");
    let result = adapter.apply_staged().await.expect("apply staged add");
    assert!(
        has_action(
            &result.delta.lifecycle_actions,
            "lifecycle-add",
            ToolConfigChangeOperation::Add,
            McpLifecyclePhase::Pending,
        ),
        "expected add/pending canonical delta, got: {:?}",
        result.delta.lifecycle_actions
    );

    wait_for_adapter_action(
        &adapter,
        "lifecycle-add",
        ToolConfigChangeOperation::Add,
        McpLifecyclePhase::Applied,
        Duration::from_secs(35),
    )
    .await;

    adapter
        .stage_reload("lifecycle-add")
        .await
        .expect("stage reload");
    let result = adapter.apply_staged().await.expect("apply staged reload");
    assert!(
        has_action(
            &result.delta.lifecycle_actions,
            "lifecycle-add",
            ToolConfigChangeOperation::Reload,
            McpLifecyclePhase::Pending,
        ),
        "expected reload/pending canonical delta, got: {:?}",
        result.delta.lifecycle_actions
    );

    wait_for_adapter_action(
        &adapter,
        "lifecycle-add",
        ToolConfigChangeOperation::Reload,
        McpLifecyclePhase::Applied,
        Duration::from_secs(35),
    )
    .await;

    adapter.shutdown().await;
}

#[tokio::test]
#[ignore = "integration-real: requires mcp-test-server binary and real process spawning"]
#[allow(clippy::expect_used, clippy::unwrap_used)]
async fn lifecycle_contract_remove_emits_draining_then_applied_and_forced_actions() {
    let Some(server_path) = skip_if_no_test_server() else {
        return;
    };

    let adapter = McpRouterAdapter::new(McpRouter::new());
    adapter
        .stage_add(test_server_config("lifecycle-drain", &server_path))
        .await
        .expect("stage add drain server");
    adapter.apply_staged().await.expect("apply staged add");
    wait_for_adapter_action(
        &adapter,
        "lifecycle-drain",
        ToolConfigChangeOperation::Add,
        McpLifecyclePhase::Applied,
        Duration::from_secs(35),
    )
    .await;

    adapter
        .set_removal_timeout_for_testing(Duration::from_secs(3))
        .await
        .expect("set drain timeout");
    adapter
        .set_inflight_calls_for_testing("lifecycle-drain", 1)
        .await
        .expect("set inflight count");
    adapter
        .stage_remove("lifecycle-drain")
        .await
        .expect("stage remove");
    let result = adapter.apply_staged().await.expect("apply staged remove");
    assert!(
        has_action(
            &result.delta.lifecycle_actions,
            "lifecycle-drain",
            ToolConfigChangeOperation::Remove,
            McpLifecyclePhase::Draining,
        ),
        "expected remove/draining canonical delta, got: {:?}",
        result.delta.lifecycle_actions
    );

    adapter
        .set_inflight_calls_for_testing("lifecycle-drain", 0)
        .await
        .expect("clear inflight count");
    let delta = adapter
        .progress_removals()
        .await
        .expect("progress removals");
    assert!(
        has_action(
            &delta.lifecycle_actions,
            "lifecycle-drain",
            ToolConfigChangeOperation::Remove,
            McpLifecyclePhase::Applied,
        ),
        "expected remove/applied canonical delta, got: {:?}",
        delta.lifecycle_actions
    );

    let forced_adapter = McpRouterAdapter::new(McpRouter::new());
    forced_adapter
        .stage_add(test_server_config("lifecycle-force", &server_path))
        .await
        .expect("stage add force server");
    forced_adapter
        .apply_staged()
        .await
        .expect("apply staged force add");
    wait_for_adapter_action(
        &forced_adapter,
        "lifecycle-force",
        ToolConfigChangeOperation::Add,
        McpLifecyclePhase::Applied,
        Duration::from_secs(35),
    )
    .await;

    forced_adapter
        .set_removal_timeout_for_testing(Duration::from_millis(10))
        .await
        .expect("set short timeout");
    forced_adapter
        .set_inflight_calls_for_testing("lifecycle-force", 1)
        .await
        .expect("set inflight count");
    forced_adapter
        .stage_remove("lifecycle-force")
        .await
        .expect("stage forced remove");
    let result = forced_adapter
        .apply_staged()
        .await
        .expect("apply forced remove");
    assert!(
        has_action(
            &result.delta.lifecycle_actions,
            "lifecycle-force",
            ToolConfigChangeOperation::Remove,
            McpLifecyclePhase::Draining,
        ),
        "expected remove/draining canonical delta, got: {:?}",
        result.delta.lifecycle_actions
    );

    tokio::time::sleep(Duration::from_millis(30)).await;
    let delta = forced_adapter
        .progress_removals()
        .await
        .expect("progress forced removal");
    assert_eq!(delta.degraded_removals, vec!["lifecycle-force"]);
    assert!(
        has_action(
            &delta.lifecycle_actions,
            "lifecycle-force",
            ToolConfigChangeOperation::Remove,
            McpLifecyclePhase::Forced,
        ),
        "expected remove/forced canonical delta, got: {:?}",
        delta.lifecycle_actions
    );

    adapter.shutdown().await;
    forced_adapter.shutdown().await;
}

#[tokio::test]
#[ignore = "integration-real: real process spawning verifies failure projection through the legacy notice bridge"]
#[allow(clippy::expect_used, clippy::unwrap_used)]
async fn lifecycle_contract_failed_activation_uses_failed_phase_and_legacy_notice_projection() {
    let mut router = McpRouter::new();
    router.stage_add(invalid_server_config("lifecycle-fail"));
    let result = router
        .apply_staged()
        .await
        .expect("apply failed staged add");
    assert!(
        has_action(
            &result.delta.lifecycle_actions,
            "lifecycle-fail",
            ToolConfigChangeOperation::Add,
            McpLifecyclePhase::Pending,
        ),
        "expected add/pending canonical delta, got: {:?}",
        result.delta.lifecycle_actions
    );

    wait_for_router_action(
        &mut router,
        "lifecycle-fail",
        ToolConfigChangeOperation::Add,
        McpLifecyclePhase::Failed,
        Duration::from_secs(5),
    )
    .await;

    let failed_adapter = McpRouterAdapter::new(McpRouter::new());
    failed_adapter
        .stage_add(invalid_server_config("lifecycle-fail-legacy"))
        .await
        .expect("stage add legacy failure");
    failed_adapter
        .apply_staged()
        .await
        .expect("apply staged legacy failure");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let update = failed_adapter.poll_external_updates().await;
        if update.notices.iter().any(|notice| {
            notice.target == "lifecycle-fail-legacy"
                && notice.operation == ToolConfigChangeOperation::Add
                && notice.status_text().starts_with("failed")
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for failed legacy notice; last update: {:?}",
            update.notices
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    failed_adapter.shutdown().await;
}
