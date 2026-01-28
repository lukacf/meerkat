use meerkat_core::AgentToolDispatcher;
use meerkat_tools::builtin::{
    BuiltinToolConfig, CompositeDispatcher, MemoryTaskStore, ToolPolicyLayer,
};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn test_rct_contracts_tool_dispatcher_contract() -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();

    let dispatcher = CompositeDispatcher::new(store, &config, None, None, None)?;

    let tools = dispatcher.tools();
    assert!(!tools.is_empty());
    Ok(())
}

#[test]
fn test_rct_contracts_agent_list_schema_contract() -> Result<(), Box<dyn std::error::Error>> {
    let config = BuiltinToolConfig {
        policy: ToolPolicyLayer::new().enable_tool("agent_list"),
        ..Default::default()
    };

    let encoded = serde_json::to_value(&config)?;
    let decoded: BuiltinToolConfig = serde_json::from_value(encoded)?;
    assert!(decoded.policy.enable.contains("agent_list"));
    Ok(())
}

#[tokio::test]
async fn test_rct_contracts_task_store_persistence_contract()
-> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();
    let tool = CompositeDispatcher::new(store, &config, None, None, None)?;

    let result = tool
        .dispatch(
            "task_create",
            &json!({"subject":"Test","description":"Persist"}),
        )
        .await?;

    assert!(result.get("id").is_some());
    Ok(())
}

#[tokio::test]
async fn test_rct_contracts_inv_004_task_tools_session_id() -> Result<(), Box<dyn std::error::Error>>
{
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();
    let tool =
        CompositeDispatcher::new(store, &config, None, None, Some("test-session-123".into()))?;

    let result = tool
        .dispatch(
            "task_create",
            &json!({"subject":"Task","description":"Track session"}),
        )
        .await?;

    assert_eq!(
        result.get("created_by_session").and_then(|v| v.as_str()),
        Some("test-session-123")
    );
    Ok(())
}

#[test]
fn test_rct_contracts_all_builtin_schemas_have_required_field()
-> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();
    let dispatcher = CompositeDispatcher::new(store, &config, None, None, None)?;

    for tool in dispatcher.tools() {
        let schema = &tool.input_schema;
        assert_eq!(schema["type"], "object");
        let _props = schema.get("properties").ok_or("missing properties")?;
    }
    Ok(())
}

#[test]
fn test_rct_contracts_inv_007_builtin_task_persistence_strategy()
-> Result<(), Box<dyn std::error::Error>> {
    // Contract: Builtin task storage strategy must be configurable
    let store = Arc::new(MemoryTaskStore::new());
    let config = BuiltinToolConfig::default();

    let dispatcher = CompositeDispatcher::new(store, &config, None, None, None)?;

    assert!(dispatcher.is_builtin("task_create"));
    Ok(())
}

#[test]
fn test_rct_contracts_shell_defaults_contract() -> Result<(), Box<dyn std::error::Error>> {
    use meerkat_tools::builtin::shell::ShellConfig;
    use std::path::PathBuf;

    let tool = ShellConfig {
        enabled: true,
        default_timeout_secs: 30,
        restrict_to_project: true,
        shell: "nu".into(),
        shell_path: None,
        project_root: PathBuf::from("/tmp"),
        max_completed_jobs: 10,
        completed_job_ttl_secs: 60,
        max_concurrent_processes: 5,
    };
    let json_str = serde_json::to_string(&tool)?;
    let json_val: serde_json::Value = serde_json::from_str(&json_str)?;
    assert_eq!(json_val["shell"], "nu");
    Ok(())
}
