//! Cross-process contract for the MCP configuration mutation lock.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use meerkat_core::mcp_config::{McpConfig, McpConfigMutationAuthority, McpServerConfig};

const CHILD_ENV: &str = "MEERKAT_MCP_CONFIG_CROSSPROC_CHILD";
const ROOT_ENV: &str = "MEERKAT_MCP_CONFIG_CROSSPROC_ROOT";
const SERVER_ENV: &str = "MEERKAT_MCP_CONFIG_CROSSPROC_SERVER";

#[tokio::test]
async fn mcp_config_crossproc_add_child() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }

    let root = PathBuf::from(std::env::var_os(ROOT_ENV).expect("child root"));
    let server_name = std::env::var(SERVER_ENV).expect("child server name");
    let authority = McpConfigMutationAuthority::project(Some(root), None);
    McpConfig::persist_add_with_rollback(
        &authority,
        McpServerConfig::stdio(server_name, "echo", Vec::new(), HashMap::new()),
    )
    .await
    .expect("cross-process add");
}

#[tokio::test]
async fn mcp_config_cross_process_distinct_adds_survive() {
    if std::env::var_os(CHILD_ENV).is_some() {
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let executable = std::env::current_exe().unwrap();
    let server_names = (0..6)
        .map(|index| format!("server-{index}"))
        .collect::<Vec<_>>();

    let mut children = Vec::new();
    for server_name in &server_names {
        let executable = executable.clone();
        let root = root.clone();
        let server_name = server_name.clone();
        children.push(tokio::task::spawn_blocking(move || {
            Command::new(executable)
                .arg("--exact")
                .arg("mcp_config_crossproc_add_child")
                .arg("--nocapture")
                .env(CHILD_ENV, "1")
                .env(ROOT_ENV, root)
                .env(SERVER_ENV, server_name)
                .status()
        }));
    }

    for child in children {
        let status = child.await.unwrap().unwrap();
        assert!(status.success(), "cross-process add child failed: {status}");
    }

    let path = temp.path().join(".rkat/mcp.toml");
    let mut persisted_names = McpConfig::load_from_paths(None, Some(&path))
        .await
        .unwrap()
        .servers
        .into_iter()
        .map(|server| server.name)
        .collect::<Vec<_>>();
    persisted_names.sort_unstable();
    assert_eq!(persisted_names, server_names);
}
