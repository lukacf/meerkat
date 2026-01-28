use std::collections::HashMap;
use std::path::PathBuf;

use meerkat::{Config, JsonlStore, McpConnection, McpServerConfig, SessionId, SessionStore};
use serde_json::json;
use tempfile::TempDir;
use tokio::time::{Duration, timeout};

fn mcp_binary_path() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir.parent()?;
    let debug = workspace_root.join("target/debug/meerkat-mcp-server");
    if debug.exists() {
        return Some(debug);
    }
    let release = workspace_root.join("target/release/meerkat-mcp-server");
    if release.exists() {
        return Some(release);
    }
    None
}

fn skip_if_no_prereqs() -> bool {
    let mut missing = Vec::new();

    if mcp_binary_path().is_none() {
        missing.push("meerkat-mcp-server binary (build with cargo build -p meerkat-mcp-server)");
    }

    if missing.is_empty() {
        return false;
    }

    eprintln!("Skipping: missing {}", missing.join(" and "));
    true
}

#[tokio::test]
async fn e2e_mcp_resume_metadata() {
    if skip_if_no_prereqs() {
        return;
    }

    let temp_dir = TempDir::new().expect("temp dir");
    let project_root = temp_dir.path().join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).expect("create .rkat");

    let base_config = Config::default();
    let store_dir = temp_dir.path().join("sessions");
    std::fs::write(
        project_root.join(".rkat/config.toml"),
        format!(
            "{}\n\n[store]\nsessions_path = \"{}\"\n",
            Config::template_toml(),
            store_dir.display()
        ),
    )
    .expect("write config");

    let mut env = HashMap::new();
    env.insert("RKAT_TEST_CLIENT".to_string(), "1".to_string());

    let mcp_bin = mcp_binary_path().expect("mcp server binary");
    let server_config = McpServerConfig::stdio("meerkat", mcp_bin.to_string_lossy(), vec![], env);
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&project_root).expect("set cwd");
    let connection = timeout(
        Duration::from_secs(20),
        McpConnection::connect(&server_config),
    )
    .await
    .expect("connect timed out")
    .expect("connect to mcp server");
    std::env::set_current_dir(&original_dir).expect("restore cwd");

    let tools = timeout(Duration::from_secs(20), connection.list_tools())
        .await
        .expect("list tools timed out")
        .expect("list tools");
    assert!(
        tools.iter().any(|t| t.name == "meerkat_run"),
        "meerkat_run tool should exist"
    );
    assert!(
        tools.iter().any(|t| t.name == "meerkat_resume"),
        "meerkat_resume tool should exist"
    );

    let run_input = json!({
        "prompt": "Say the word 'ok' and nothing else.",
        "model": base_config.agent.model,
        "max_tokens": base_config.agent.max_tokens_per_turn,
        "tools": []
    });

    let run_response = timeout(
        Duration::from_secs(120),
        connection.call_tool("meerkat_run", &run_input),
    )
    .await
    .expect("meerkat_run timed out")
    .expect("meerkat_run");
    let run_json: serde_json::Value =
        serde_json::from_str(&run_response).expect("run response json");
    let session_id = run_json["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();

    let store = JsonlStore::new(store_dir);
    store.init().await.expect("init store");
    let session = store
        .load(&SessionId::parse(&session_id).expect("session id"))
        .await
        .expect("load session")
        .expect("session exists");
    let metadata = session.session_metadata().expect("metadata");

    let original_model = metadata.model.clone();
    let original_max_tokens = metadata.max_tokens;
    let original_tooling = metadata.tooling.clone();
    let original_provider = metadata.provider;

    let resume_input = json!({
        "session_id": session_id,
        "prompt": "Continue.",
        "tools": []
    });

    let _ = timeout(
        Duration::from_secs(120),
        connection.call_tool("meerkat_resume", &resume_input),
    )
    .await
    .expect("meerkat_resume timed out")
    .expect("meerkat_resume");

    let session = store
        .load(&SessionId::parse(run_json["session_id"].as_str().unwrap()).expect("session id"))
        .await
        .expect("load session")
        .expect("session exists");
    let metadata = session.session_metadata().expect("metadata");

    assert_eq!(metadata.model, original_model, "model should persist");
    assert_eq!(
        metadata.max_tokens, original_max_tokens,
        "max_tokens should persist"
    );
    assert_eq!(
        metadata.provider, original_provider,
        "provider should persist"
    );
    assert_eq!(metadata.tooling.builtins, original_tooling.builtins);
    assert_eq!(metadata.tooling.shell, original_tooling.shell);
    assert_eq!(metadata.tooling.comms, original_tooling.comms);
    assert_eq!(metadata.tooling.subagents, original_tooling.subagents);

    connection.close().await.expect("close connection");
}
