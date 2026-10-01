#![allow(clippy::expect_used)]
//! Bounded baseline observation: two routers share the actual session owner,
//! but each router's execution payload map belongs to that router alone.
//! This is not a successful two-account MCP composition or a proxy test.

use std::sync::Arc;

use meerkat_core::{
    AgentToolDispatcher, ExternalToolSurfacePendingOp, ExternalToolSurfaceStagedOp,
    McpServerConfig, SessionId,
};
use meerkat_mcp::{McpRouter, McpRouterAdapter};
use meerkat_runtime::{MeerkatMachine, session_runtime_bindings_have_machine_authority};

#[tokio::test]
async fn sibling_router_cannot_apply_another_accounts_shared_session_intent() {
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let bindings = machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("real session binding is a fixture prerequisite");
    assert_eq!(bindings.session_id(), &session_id);
    assert!(session_runtime_bindings_have_machine_authority(&bindings));

    let work = McpRouterAdapter::new(McpRouter::new());
    let personal = McpRouterAdapter::new(McpRouter::new());
    for adapter in [&work, &personal] {
        // These are the exact same canonical handles the factory and dynamic
        // dispatcher composition bind; no standalone or filtered owner exists.
        adapter.bind_external_tool_surface_handle(Arc::clone(bindings.external_tool_surface()));
        adapter.bind_mcp_server_lifecycle_handle(Arc::clone(bindings.mcp_server_lifecycle()));
    }
    work.apply_staged()
        .await
        .expect("empty shared owner must admit the initial boundary");
    let staged_name = "personal__gmail";
    personal
        .stage_add(McpServerConfig::stdio(
            staged_name,
            "/m6-fixture/no-process-is-started",
            vec![],
            Default::default(),
        ))
        .await
        .expect("the actual session must accept the personal account's staged add");
    let before = bindings
        .external_tool_surface()
        .surface_snapshot(staged_name)
        .expect("accepted add must be visible on the actual session handle");

    let result = work.apply_staged().await;
    let after = bindings
        .external_tool_surface()
        .surface_snapshot(staged_name);

    // Never apply the personal router's own intent in this fixture. The
    // observed failure precedes ApplyBoundary and connect/enumerate. Retire
    // both empty adapters before asserting the captured counterexample.
    work.shutdown().await;
    personal.shutdown().await;

    let error = result.expect_err("the sibling lacks the staged account's local payload");
    assert_eq!(
        error,
        "Protocol error: staged add for 'personal__gmail' is missing its staged payload"
    );
    assert_eq!(before.staged_op, ExternalToolSurfaceStagedOp::Add);
    assert!(before.staged_intent_sequence.is_some());
    assert_eq!(before.pending_op, ExternalToolSurfacePendingOp::None);
    assert_eq!(before.pending_task_sequence, None);
    assert_eq!(
        after,
        Some(before),
        "the foreign boundary must not consume the intent"
    );
}
