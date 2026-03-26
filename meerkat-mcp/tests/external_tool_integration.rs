#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 1 red-ok tests for typed external-tool lifecycle notices.

use std::collections::HashMap;
use std::time::Duration;

use meerkat_core::McpServerConfig;
use meerkat_core::agent::AgentToolDispatcher;
use meerkat_core::event::ToolConfigChangeOperation;
use meerkat_mcp::{McpLifecyclePhase, McpRouter, McpRouterAdapter};

fn invalid_server_config(name: &str) -> McpServerConfig {
    McpServerConfig::stdio(
        name,
        "/definitely/not/a/real/mcp-server".to_string(),
        vec![],
        HashMap::new(),
    )
}

#[tokio::test]
#[ignore = "Phase 1 red-ok external-tool integration suite"]
async fn external_tool_integration_red_ok_failed_add_surfaces_typed_notice_update() {
    let adapter = McpRouterAdapter::new(McpRouter::new());
    adapter
        .stage_add(invalid_server_config("phase1-invalid"))
        .await
        .expect("stage invalid server");
    let result = adapter.apply_staged().await.expect("apply staged add");

    assert!(
        result.delta.lifecycle_actions.iter().any(|action| {
            action.target == "phase1-invalid"
                && action.operation == ToolConfigChangeOperation::Add
                && action.phase == McpLifecyclePhase::Pending
        }),
        "staged add should emit a pending typed lifecycle action first"
    );

    let mut notices = Vec::new();
    for _ in 0..20 {
        let update = adapter.poll_external_updates().await;
        notices.extend(update.notices);
        if notices
            .iter()
            .any(|notice| notice.target == "phase1-invalid")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let failure = notices
        .iter()
        .find(|notice| notice.target == "phase1-invalid")
        .expect("background failure should surface as a typed external-tool notice");
    assert_eq!(failure.operation, ToolConfigChangeOperation::Add);
    assert!(
        failure.status_text().starts_with("failed"),
        "failed MCP startup should project a typed failure notice"
    );
    assert_eq!(failure.tool_count, None);

    adapter.shutdown().await;
}
