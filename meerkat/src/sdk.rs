//! SDK helper functions for tool dispatcher setup.

use std::sync::Arc;

use crate::AgentToolDispatcher;
use meerkat_tools::builtin::shell::ShellConfig;
use meerkat_tools::{
    BuiltinDispatcherConfig, BuiltinToolConfig, CompositeDispatcherError, FileTaskStore,
    build_builtin_dispatcher, ensure_rkat_dir, find_project_root,
};

/// Create a tool dispatcher with built-in tools enabled.
///
/// This is a convenience function for setting up an agent with Meerkat's
/// built-in task management tools. It automatically:
/// - Detects the project root (using `.rkat/` or `.git/` markers)
/// - Creates the `.rkat/` directory if needed
/// - Sets up a file-based task store
/// - Creates a composite dispatcher with the configured built-in tools
///
/// # Arguments
/// * `config` - Configuration for enabling/disabling built-in tools
/// * `external` - Optional external dispatcher for additional tools (e.g., MCP router)
/// * `session_id` - Optional session ID for tracking tool usage
///
/// # Returns
/// An `Arc<dyn AgentToolDispatcher>` ready to use with `AgentBuilder::build()`
pub fn create_dispatcher_with_builtins(
    config: &BuiltinToolConfig,
    shell_config: Option<ShellConfig>,
    external: Option<Arc<dyn AgentToolDispatcher>>,
    session_id: Option<String>,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    let cwd = std::env::current_dir().map_err(CompositeDispatcherError::Io)?;
    let project_root = find_project_root(&cwd);
    ensure_rkat_dir(&project_root).map_err(CompositeDispatcherError::Io)?;
    let store = Arc::new(FileTaskStore::in_project(&project_root));
    let builder = BuiltinDispatcherConfig {
        store,
        config: config.clone(),
        shell_config,
        external,
        session_id,
        enable_wait_interrupt: false,
    };
    build_builtin_dispatcher(builder)
}

/// Create a tool dispatcher with only built-in task tools (no shell tools, no external tools).
///
/// This is a convenience wrapper around [`create_dispatcher_with_builtins`].
pub fn create_builtins_dispatcher(
    config: &BuiltinToolConfig,
    session_id: Option<String>,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    create_dispatcher_with_builtins(config, None, None, session_id)
}

/// Create a tool dispatcher with built-in task and shell tools.
pub fn create_shell_dispatcher(
    config: &BuiltinToolConfig,
    shell_config: ShellConfig,
    session_id: Option<String>,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    create_dispatcher_with_builtins(config, Some(shell_config), None, session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_tools::MemoryTaskStore;

    #[tokio::test]
    async fn test_builtin_tools_dispatch() {
        use std::sync::Arc;

        let store = Arc::new(MemoryTaskStore::new());
        let config = BuiltinToolConfig::default();
        let builder = BuiltinDispatcherConfig {
            store,
            config,
            shell_config: None,
            external: None,
            session_id: None,
            enable_wait_interrupt: false,
        };
        let dispatcher = build_builtin_dispatcher(builder).unwrap();

        // Create a task
        let args = serde_json::json!({
            "subject": "Integration test task",
            "description": "Testing the builtin dispatcher"
        });
        let result = dispatcher.dispatch("task_create", &args).await;
        assert!(result.is_ok());

        let task = result.unwrap();
        assert!(task.get("id").is_some());
        assert_eq!(task.get("subject").unwrap(), "Integration test task");

        // List tasks - returns an array directly
        let list_result = dispatcher
            .dispatch("task_list", &serde_json::json!({}))
            .await;
        assert!(list_result.is_ok());
        let list = list_result.unwrap();
        assert!(list.is_array());
        let tasks = list.as_array().unwrap();
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn test_create_dispatcher_in_project_dir() {
        // Test the helper function (uses tempdir to avoid polluting the workspace)
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        // Create a .rkat directory to mark it as a project
        std::fs::create_dir_all(temp_path.join(".rkat")).unwrap();

        // Change to temp dir for this test
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_path).unwrap();

        // Create dispatcher using the helper
        let result =
            create_builtins_dispatcher(&BuiltinToolConfig::default(), Some("test-123".to_string()));

        // Restore original dir
        std::env::set_current_dir(original_dir).unwrap();

        assert!(result.is_ok());
        let dispatcher = result.unwrap();
        let tools = dispatcher.tools();
        // 4 task tools + 2 utility tools (wait, datetime)
        assert!(tools.iter().any(|t| t.name == "task_create"));
        assert!(tools.iter().any(|t| t.name == "wait"));
    }

    #[tokio::test]
    async fn test_builtin_tools_in_project_dir() {
        use std::sync::Arc;

        // Test using FileTaskStore in a temp directory
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        // Create the .rkat directory
        ensure_rkat_dir(temp_path).unwrap();

        // Create dispatcher with FileTaskStore
        let store = Arc::new(FileTaskStore::in_project(temp_path));
        let config = BuiltinToolConfig::default();
        let builder = BuiltinDispatcherConfig {
            store,
            config,
            shell_config: None,
            external: None,
            session_id: Some("file-test-session".to_string()),
            enable_wait_interrupt: false,
        };
        let dispatcher = build_builtin_dispatcher(builder).unwrap();

        // Create a task
        let create_result = dispatcher
            .dispatch(
                "task_create",
                &serde_json::json!({
                    "subject": "File store test",
                    "description": "Testing with real file storage"
                }),
            )
            .await;
        assert!(create_result.is_ok());

        let task = create_result.unwrap();
        let task_id = task.get("id").unwrap().as_str().unwrap();
        assert_eq!(task.get("created_by_session").unwrap(), "file-test-session");

        // Verify tasks.json was created in .rkat directory
        let tasks_file = temp_path.join(".rkat").join("tasks.json");
        assert!(tasks_file.exists(), "tasks.json should be created");

        // Get the task back
        let get_result = dispatcher
            .dispatch("task_get", &serde_json::json!({"id": task_id}))
            .await;
        assert!(get_result.is_ok());
        let retrieved = get_result.unwrap();
        assert_eq!(retrieved.get("subject").unwrap(), "File store test");
    }
}
