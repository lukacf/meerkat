//! A member's in-process file tools and the gateway's own directory.
//!
//! The gateway builds its members' tools through `AgentFactory` with the
//! workspace as their project root (`meta_gateway`: project root and runtime
//! root default to the workspace, the store to `<workspace>/state`, built-in
//! tools on). `<workspace>/.rkat/` holds the gateway's Meerkat host config
//! (`meerkat_config_path` defaults to `<workspace>/.rkat/config.toml`) and
//! the comms identity keys. Meerkat's file tools refuse that directory with
//! no declaration from the host, so a member's `apply_patch` cannot rewrite
//! the config its gateway starts every agent from. Every other workspace
//! file stays writable, the gateway's undeclared `state` directory included.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use meerkat::{AgentFactory, BuiltinToolConfig, MemoryTaskStore};
use meerkat_core::types::ToolCallView;
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome, ToolError};

/// The builtin dispatcher of a member built the way the gateway builds it.
async fn member_dispatcher(workspace: &Path) -> Arc<dyn AgentToolDispatcher> {
    AgentFactory::new(workspace.join("state"))
        .runtime_root(workspace)
        .project_root(workspace)
        .context_root(workspace)
        .builtins(true)
        .shell(true)
        .build_builtin_dispatcher(
            Arc::new(MemoryTaskStore::new()),
            BuiltinToolConfig::default(),
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("a member's builtin dispatcher builds")
}

/// One `apply_patch` add of `path` through the member's dispatcher.
async fn add_file(
    dispatcher: &dyn AgentToolDispatcher,
    path: &str,
) -> Result<ToolDispatchOutcome, ToolError> {
    let args = serde_json::json!({
        "patch": format!("*** Begin Patch\n*** Add File: {path}\n+written by a member\n*** End Patch")
    });
    let raw = serde_json::value::RawValue::from_string(args.to_string()).unwrap();
    dispatcher
        .dispatch(ToolCallView {
            id: "member-file-tool",
            name: "apply_patch",
            args: &raw,
        })
        .await
}

#[tokio::test]
async fn a_members_file_tool_cannot_write_the_workspace_engine_directory() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().canonicalize().unwrap();
    std::fs::create_dir_all(workspace.join(".rkat")).unwrap();
    std::fs::write(workspace.join(".rkat/config.toml"), "kept = true\n").unwrap();
    let dispatcher = member_dispatcher(&workspace).await;

    for path in [
        ".rkat/mcp.toml",
        ".rkat/session_comms_identity/member/key",
        ".RKAT/config.toml",
    ] {
        let result = add_file(dispatcher.as_ref(), path).await;
        assert!(
            matches!(
                &result,
                Err(ToolError::InvalidArguments { name, reason })
                    if name == "apply_patch" && reason.contains("engine's own directory")
            ),
            "{path}: expected the typed refusal, got {result:?}"
        );
        assert!(!workspace.join(path).exists(), "{path} was written");
    }
    let update = serde_json::json!({
        "patch": "*** Begin Patch\n*** Update File: .rkat/config.toml\n@@\n-kept = true\n+kept = false\n*** End Patch"
    });
    let raw = serde_json::value::RawValue::from_string(update.to_string()).unwrap();
    let result = dispatcher
        .dispatch(ToolCallView {
            id: "member-file-tool",
            name: "apply_patch",
            args: &raw,
        })
        .await;
    assert!(
        matches!(&result, Err(ToolError::InvalidArguments { .. })),
        "an update of the gateway's host config: {result:?}"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join(".rkat/config.toml")).unwrap(),
        "kept = true\n"
    );

    // Ordinary workspace files, and the gateway's undeclared state
    // directory, are written as before.
    for path in ["notes/plan.md", "src/lib.rs", "state/scratch.txt"] {
        add_file(dispatcher.as_ref(), path)
            .await
            .unwrap_or_else(|error| panic!("{path} is an ordinary workspace file: {error}"));
        assert_eq!(
            std::fs::read_to_string(workspace.join(path)).unwrap(),
            "written by a member\n"
        );
    }
}
