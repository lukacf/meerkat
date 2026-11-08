use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use meerkat_core::AgentToolDispatcher;
use meerkat_core::McpServerConfig;
use meerkat_core::event::ToolConfigChangeOperation;
use meerkat_core::handles::ExternalToolSurfaceHandle;
use meerkat_mcp::{McpLifecycleAction, McpLifecyclePhase, McpRouter, McpRouterAdapter};
use meerkat_runtime::RuntimeExternalToolSurfaceHandle;

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

fn generated_surface_handle() -> Arc<dyn ExternalToolSurfaceHandle> {
    Arc::new(RuntimeExternalToolSurfaceHandle::ephemeral())
}

fn generated_router() -> McpRouter {
    McpRouter::new_with_surface_handle_and_stdio_profile(
        generated_surface_handle(),
        meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
    )
}

fn generated_adapter() -> McpRouterAdapter {
    McpRouterAdapter::new(generated_router())
}

fn has_action(
    actions: &[McpLifecycleAction],
    target: &str,
    operation: ToolConfigChangeOperation,
    phase: McpLifecyclePhase,
) -> bool {
    let expected_settled = !matches!(
        phase,
        McpLifecyclePhase::Pending | McpLifecyclePhase::Draining
    );
    actions.iter().any(|action| {
        action.target == target
            && action.operation == operation
            && action.phase == phase
            && action.settled() == expected_settled
            && action.applied_at_turn.is_none()
    })
}

#[test]
#[allow(clippy::expect_used)]
fn standalone_router_with_generated_surface_accepts_surface_stage_add() {
    let mut router = generated_router();

    router
        .stage_add(invalid_server_config("standalone-stage-add"))
        .expect("standalone router should accept staged MCP lifecycle input");

    let snapshot = router.external_tool_surface_snapshot();
    assert_eq!(snapshot.entries.len(), 1);

    let entry = &snapshot.entries[0];
    assert_eq!(entry.surface_id, "standalone-stage-add");
    assert_eq!(
        entry.staged_op,
        meerkat_core::ExternalToolSurfaceStagedOp::Add
    );
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
#[allow(clippy::expect_used, clippy::unwrap_used)]
async fn lifecycle_contract_add_and_reload_emit_pending_then_applied_actions() {
    let server_path = mcp_test_server::fixture_binary();

    let adapter = generated_adapter();

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
#[allow(clippy::expect_used, clippy::unwrap_used)]
async fn lifecycle_contract_remove_emits_draining_then_applied_and_forced_actions() {
    let server_path = mcp_test_server::fixture_binary();

    let adapter = generated_adapter();
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

    let forced_adapter = generated_adapter();
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
    let mut router = generated_router();
    router
        .stage_add(invalid_server_config("lifecycle-fail"))
        .expect("stage invalid server");
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

    let failed_adapter = generated_adapter();
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

#[tokio::test]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
async fn observation_reports_the_current_route_and_accepted_state() {
    use meerkat_core::McpOperationObservation as Observation;
    let server_path = mcp_test_server::fixture_binary();
    let adapter = generated_adapter();

    assert_eq!(
        adapter.observe_operation("observed", "echo").await,
        Observation::Absent
    );
    adapter
        .stage_add(test_server_config("observed", &server_path))
        .await
        .expect("stage add");
    assert_eq!(
        adapter.observe_operation("observed", "echo").await,
        Observation::Pending,
        "a staged server is never callable"
    );
    adapter.apply_staged().await.expect("apply staged add");
    wait_for_adapter_action(
        &adapter,
        "observed",
        ToolConfigChangeOperation::Add,
        McpLifecyclePhase::Applied,
        Duration::from_secs(35),
    )
    .await;
    adapter.refresh_tools().await.expect("refresh");
    let Observation::Callable {
        connection_generation: first,
        connector: None,
        service_secret_launch: None,
    } = adapter.observe_operation("observed", "echo").await
    else {
        panic!("the connected server's route is callable");
    };
    assert_eq!(
        adapter
            .observe_operation("observed", "not-an-operation")
            .await,
        Observation::Absent,
        "an operation the server does not expose is absent"
    );

    // A staged replacement coexists with the live route it will replace.
    adapter
        .stage_reload("observed")
        .await
        .expect("stage reload");
    assert_eq!(
        adapter.observe_operation("observed", "echo").await,
        Observation::Callable {
            connection_generation: first,
            connector: None,
            service_secret_launch: None,
        },
        "the current route stays callable while its replacement is staged"
    );
    adapter.apply_staged().await.expect("apply staged reload");
    wait_for_adapter_action(
        &adapter,
        "observed",
        ToolConfigChangeOperation::Reload,
        McpLifecyclePhase::Applied,
        Duration::from_secs(35),
    )
    .await;
    adapter.refresh_tools().await.expect("refresh");
    match adapter.observe_operation("observed", "echo").await {
        Observation::Callable {
            connection_generation,
            ..
        } => assert_ne!(
            connection_generation, first,
            "the replacement connection serves it now"
        ),
        other => panic!("the reloaded server is callable: {other:?}"),
    }

    // An accepted failure is read from current state after its notices drained.
    adapter
        .stage_add(invalid_server_config("broken"))
        .await
        .expect("stage broken add");
    adapter
        .apply_staged()
        .await
        .expect("apply staged broken add");
    wait_for_adapter_action(
        &adapter,
        "broken",
        ToolConfigChangeOperation::Add,
        McpLifecyclePhase::Failed,
        Duration::from_secs(35),
    )
    .await;
    let _ = adapter.poll_lifecycle_actions().await;
    assert_eq!(
        adapter.observe_operation("broken", "echo").await,
        Observation::Failed
    );

    adapter.shutdown().await;
    assert_eq!(
        adapter.observe_operation("observed", "echo").await,
        Observation::Retired,
        "a router taken for shutdown is retired"
    );
}

/// A router that borrows its surface releases only what it staged when it
/// shuts down, so the surface stays open for the next holder; a router that
/// owns its surface shuts the whole surface down.
#[tokio::test]
#[allow(clippy::expect_used)]
async fn shutdown_releases_a_borrowed_surface_and_closes_an_owned_one() {
    use meerkat_core::{ExternalToolSurfaceBaseState, ExternalToolSurfaceGlobalPhase};
    use meerkat_mcp::McpSurfaceOwnership;
    let server_path = mcp_test_server::fixture_binary();
    for ownership in [McpSurfaceOwnership::Borrowed, McpSurfaceOwnership::Owned] {
        let surface = generated_surface_handle();
        let adapter = McpRouterAdapter::new(
            McpRouter::new_with_surface_handle_and_stdio_profile(
                Arc::clone(&surface),
                meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
            )
            .with_surface_ownership(ownership),
        );
        adapter
            .stage_add(test_server_config("held", &server_path))
            .await
            .expect("stage add");
        adapter.apply_staged().await.expect("apply staged add");
        wait_for_adapter_action(
            &adapter,
            "held",
            ToolConfigChangeOperation::Add,
            McpLifecyclePhase::Applied,
            Duration::from_secs(35),
        )
        .await;
        adapter.shutdown().await;

        let phase = surface.diagnostic_snapshot().surface_phase;
        let base = surface
            .surface_snapshot("held")
            .and_then(|entry| entry.base_state);
        match ownership {
            McpSurfaceOwnership::Borrowed => {
                assert_eq!(phase, ExternalToolSurfaceGlobalPhase::Operating);
                assert_eq!(base, Some(ExternalToolSurfaceBaseState::Removed));
                // The next holder stages the same server.
                let mut next = McpRouter::new_with_surface_handle_and_stdio_profile(
                    Arc::clone(&surface),
                    meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
                );
                next.stage_add(test_server_config("held", &server_path))
                    .expect("the next holder stages the released server");
            }
            McpSurfaceOwnership::Owned => {
                assert_eq!(phase, ExternalToolSurfaceGlobalPhase::Shutdown);
            }
        }
    }
}

/// A router built `Owned` on its private surface and later bound onto a
/// session registration's surface borrows that surface: its shutdown releases
/// only its own lineages there and leaves the registration's surface open.
#[tokio::test]
#[allow(clippy::expect_used)]
async fn a_late_bind_onto_a_registration_surface_makes_the_router_borrow_it() {
    use meerkat_core::{ExternalToolSurfaceBaseState, ExternalToolSurfaceGlobalPhase};
    use meerkat_mcp::McpSurfaceOwnership;
    let server_path = mcp_test_server::fixture_binary();
    let registration = generated_surface_handle();
    let adapter = McpRouterAdapter::new(
        McpRouter::new_with_surface_handle_and_stdio_profile(
            generated_surface_handle(),
            meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
        )
        .with_surface_ownership(McpSurfaceOwnership::Owned),
    );
    adapter.bind_external_tool_surface_handle(Arc::clone(&registration));
    adapter
        .stage_add(test_server_config("bound", &server_path))
        .await
        .expect("stage add");
    adapter.apply_staged().await.expect("apply staged add");
    wait_for_adapter_action(
        &adapter,
        "bound",
        ToolConfigChangeOperation::Add,
        McpLifecyclePhase::Applied,
        Duration::from_secs(35),
    )
    .await;
    adapter.shutdown().await;
    assert_eq!(
        registration.diagnostic_snapshot().surface_phase,
        ExternalToolSurfaceGlobalPhase::Operating,
        "the registration's surface stays open"
    );
    assert_eq!(
        registration
            .surface_snapshot("bound")
            .and_then(|entry| entry.base_state),
        Some(ExternalToolSurfaceBaseState::Removed)
    );
}

/// A stage on the grant path is held by the construction that staged it:
/// once that construction's router, borrowing a session registration's
/// surface, shuts down, it releases what it staged, applied or not, so the
/// registration's surface keeps no staged, pending or active slot for the
/// server and the next construction's gated stage succeeds. A server the
/// registration's grant gate does not gate is never staged on that path.
#[tokio::test]
#[allow(clippy::expect_used)]
async fn a_gated_stage_is_released_with_the_construction_that_staged_it() {
    use meerkat_core::{
        ExternalToolSurfaceBaseState, ExternalToolSurfacePendingOp, ExternalToolSurfaceStagedOp,
    };
    use meerkat_mcp::McpSurfaceOwnership;
    let server_path = mcp_test_server::fixture_binary();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let bindings = machine
        .prepare_bindings(meerkat_core::SessionId::new())
        .await
        .expect("runtime bindings");
    bindings
        .tool_visibility_owner()
        .replace_tool_grant_gate(meerkat_core::tool_grant::InstalledToolGrantGate {
            gated_servers: ["gated".to_string()].into_iter().collect(),
            ..Default::default()
        })
        .expect("the registration installs its gate");
    let surface = Arc::clone(bindings.external_tool_surface());
    let construction = || {
        McpRouterAdapter::new(
            McpRouter::new_with_surface_handle_and_stdio_profile(
                Arc::clone(&surface),
                meerkat_mcp::McpStdioLaunchProfile::trusted_host(),
            )
            .with_surface_ownership(McpSurfaceOwnership::Borrowed),
        )
    };

    for apply in [false, true] {
        let adapter = construction();
        adapter
            .stage_add_gated(test_server_config("gated", &server_path))
            .await
            .expect("the gate gates the server");
        if apply {
            adapter.apply_staged().await.expect("apply the gated add");
            wait_for_adapter_action(
                &adapter,
                "gated",
                ToolConfigChangeOperation::Add,
                McpLifecyclePhase::Applied,
                Duration::from_secs(35),
            )
            .await;
        }
        adapter.shutdown().await;
        let slot = surface.surface_snapshot("gated");
        assert!(
            slot.as_ref().is_none_or(|slot| {
                slot.staged_op == ExternalToolSurfaceStagedOp::None
                    && slot.pending_op == ExternalToolSurfacePendingOp::None
                    && slot
                        .base_state
                        .is_none_or(|base| base == ExternalToolSurfaceBaseState::Removed)
            }),
            "applied: {apply}: the ended construction left a slot: {slot:?}"
        );
    }

    let next = construction();
    next.stage_add_gated(test_server_config("gated", &server_path))
        .await
        .expect("the next construction stages the released server");
    assert!(
        next.stage_add_gated(test_server_config("open", &server_path))
            .await
            .is_err(),
        "a server the gate does not gate is never staged on the grant path"
    );
    next.shutdown().await;
}
