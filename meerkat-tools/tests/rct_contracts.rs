use meerkat_core::ToolError;
use meerkat_tools::builtin::shell::ShellConfig;
use meerkat_tools::builtin::shell::ShellToolSet;
use meerkat_tools::builtin::{BuiltinTool, FileTaskStore, MemoryTaskStore, TaskCreateTool};
use meerkat_tools::dispatcher::{ToolDispatcherConfig, ToolDispatcherKind};
use serde_json::json;
use std::sync::Arc;

#[test]
fn test_shell_allowlist_contract() {
    let tool_set = ShellToolSet::new(ShellConfig::default());
    let names = [
        tool_set.job_status.name(),
        tool_set.jobs_list.name(),
        tool_set.job_cancel.name(),
    ];
    assert!(names.contains(&"shell_job_status"));
    assert!(names.contains(&"shell_jobs"));
    assert!(names.contains(&"shell_job_cancel"));
}

#[test]
fn test_tool_dispatcher_contract() {
    let config = ToolDispatcherConfig::default();
    assert_eq!(config.kind, ToolDispatcherKind::Composite);

    let encoded = serde_json::to_value(&config).expect("serialize");
    let decoded: ToolDispatcherConfig = serde_json::from_value(encoded).expect("deserialize");
    assert_eq!(decoded.kind, ToolDispatcherKind::Composite);
}

#[test]
fn test_tool_error_schema_contract() {
    let err = ToolError::invalid_arguments("tool_x", "bad input");
    let message = format!("{err}");
    assert!(message.contains("tool_x"));
    assert!(message.contains("bad input"));
}

#[tokio::test]
async fn test_task_store_persistence_contract() {
    let store = Arc::new(MemoryTaskStore::new());
    let tool = TaskCreateTool::with_session(store, "sess-123".to_string());
    let result = tool
        .call(json!({"subject":"Test","description":"Persist"}))
        .await
        .expect("task create");

    assert_eq!(result["created_by_session"], "sess-123");
    assert_eq!(result["updated_by_session"], "sess-123");
}

#[tokio::test]
async fn test_inv_004_task_tools_session_id() {
    let store = Arc::new(MemoryTaskStore::new());
    let tool = TaskCreateTool::with_session(store, "sess-999".to_string());
    let result = tool
        .call(json!({"subject":"Task","description":"Track session"}))
        .await
        .expect("task create");

    assert_eq!(result["created_by_session"], "sess-999");
}

#[test]
fn test_shell_defaults_contract() {
    let core_defaults = meerkat_core::ShellDefaults::default();
    let shell_config = ShellConfig::default();
    assert_eq!(core_defaults.program, shell_config.shell);
    assert_eq!(
        core_defaults.timeout_secs,
        shell_config.default_timeout_secs
    );
    assert!(core_defaults.allowlist.is_empty());
}

#[test]
fn test_inv_007_builtin_task_persistence_strategy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = FileTaskStore::in_project(dir.path());
    let path = store.path().to_string_lossy().to_string();
    assert!(path.ends_with(".rkat/tasks.json"));
}
