#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use meerkat_core::{ContextConfig, RealmConfig, RealmSelection, RuntimeBootstrap};
use meerkat_mcp_server::{MeerkatMcpState, handle_tools_call, tools_list};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]

fn runtime_backed_ingress_red_ok_mcp_tools_list_exposes_run_and_resume_surfaces() {
    let tools = tools_list();
    assert!(tools.iter().any(|tool| tool["name"] == "meerkat_run"));
    assert!(tools.iter().any(|tool| tool["name"] == "meerkat_resume"));
}

fn unwrap_payload(value: Value) -> Value {
    let raw = value["content"][0]["text"]
        .as_str()
        .expect("wrapped MCP payload text");
    serde_json::from_str(raw).expect("wrapped payload json")
}

fn mcp_bootstrap(root: &Path, instance_id: &str) -> RuntimeBootstrap {
    let project_root = root.join("project");
    std::fs::create_dir_all(project_root.join(".rkat")).expect("project root should initialize");
    RuntimeBootstrap {
        realm: RealmConfig {
            selection: RealmSelection::Explicit {
                realm_id: "mcp-runtime-backed-ingress".to_string(),
            },
            instance_id: Some(instance_id.to_string()),
            backend_hint: None,
            state_root: Some(root.join("realms")),
        },
        context: ContextConfig {
            context_root: Some(project_root),
            user_config_root: None,
        },
    }
}

fn unique_root(prefix: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("meerkat-{prefix}-{nanos}-{}", std::process::id()))
}

#[tokio::test]

async fn runtime_backed_ingress_red_ok_mcp_run_and_resume_reuse_the_same_runtime_session() {
    let root = unique_root("mcp-run-resume");
    let state = MeerkatMcpState::new_with_bootstrap_and_test_client(
        mcp_bootstrap(&root, "runtime-backed"),
        true,
    )
    .await
    .expect("mcp state should initialize");

    let created = unwrap_payload(
        handle_tools_call(
            &state,
            "meerkat_run",
            &json!({
                "prompt": "Remember that phase nine uses runtime-backed MCP sessions.",
                "model": "claude-sonnet-4-5",
            }),
        )
        .await
        .expect("meerkat_run should succeed"),
    );
    let session_id = created["session_id"]
        .as_str()
        .expect("meerkat_run should return a session id")
        .to_string();
    assert_eq!(created["content"][0]["text"], "ok");

    let listed_after_run = unwrap_payload(
        handle_tools_call(&state, "meerkat_sessions", &json!({}))
            .await
            .expect("meerkat_sessions after run should succeed"),
    );
    assert!(
        listed_after_run["sessions"]
            .as_array()
            .is_some_and(|sessions| sessions
                .iter()
                .any(|entry| entry["session_id"].as_str() == Some(session_id.as_str()))),
        "runtime-backed run should materialize a readable session"
    );

    let resumed = unwrap_payload(
        handle_tools_call(
            &state,
            "meerkat_resume",
            &json!({
                "session_id": session_id,
                "prompt": "Confirm that resume stays on the same runtime-backed session.",
            }),
        )
        .await
        .expect("meerkat_resume should succeed"),
    );
    assert_eq!(resumed["content"][0]["text"], "ok");
    assert_eq!(
        resumed["session_id"], created["session_id"],
        "resume should target the original runtime-backed session id"
    );

    let listed_after_resume = unwrap_payload(
        handle_tools_call(&state, "meerkat_sessions", &json!({}))
            .await
            .expect("meerkat_sessions after resume should succeed"),
    );
    assert!(
        listed_after_resume["sessions"]
            .as_array()
            .is_some_and(|sessions| sessions
                .iter()
                .filter(|entry| entry["session_id"].as_str() == created["session_id"].as_str())
                .count()
                == 1),
        "resume should not create a second surface-local session entry"
    );
}

#[tokio::test]
async fn runtime_backed_ingress_mcp_run_rejects_malformed_connection_ref() {
    let root = unique_root("mcp-run-invalid-connection-ref");
    let state = MeerkatMcpState::new_with_bootstrap_and_test_client(
        mcp_bootstrap(&root, "runtime-backed"),
        true,
    )
    .await
    .expect("mcp state should initialize");

    let error = handle_tools_call(
        &state,
        "meerkat_run",
        &json!({
            "prompt": "hello",
            "connection_ref": "missing-colon",
        }),
    )
    .await
    .expect_err("malformed connection_ref must be rejected");

    assert!(
        error.message.contains("connection_ref"),
        "error should explain the malformed connection_ref: {error:?}"
    );
}

#[tokio::test]
async fn runtime_backed_ingress_mcp_run_rejects_cross_realm_connection_ref() {
    let root = unique_root("mcp-run-cross-realm-connection-ref");
    let bootstrap = mcp_bootstrap(&root, "runtime-backed");
    let config_path = bootstrap
        .context
        .context_root
        .as_ref()
        .expect("context root")
        .join(".rkat/config.toml");
    let config_toml = r#"
[realm.other-realm]
default_binding = "default_openai"

[realm.other-realm.backend.default_openai]
provider = "openai"
backend_kind = "openai_api"

[realm.other-realm.auth.default_openai]
provider = "openai"
auth_method = "api_key"

[realm.other-realm.auth.default_openai.source]
kind = "inline_secret"
secret = "sk-cross-realm"

[realm.other-realm.binding.default_openai]
backend_profile = "default_openai"
auth_profile = "default_openai"
"#;
    std::fs::write(&config_path, config_toml).expect("config file should write");

    let state = MeerkatMcpState::new_with_bootstrap_and_test_client(bootstrap, true)
        .await
        .expect("mcp state should initialize");

    let error = handle_tools_call(
        &state,
        "meerkat_run",
        &json!({
            "prompt": "hello",
            "connection_ref": "other-realm:default_openai",
        }),
    )
    .await
    .expect_err("cross-realm connection_ref must be rejected");

    assert!(
        error.message.contains("does not match active realm"),
        "error should explain the active-realm mismatch: {error:?}"
    );
}
