//! SDK helper functions for tool dispatcher setup.

use std::path::Path;
use std::sync::Arc;

use crate::AgentToolDispatcher;
use crate::{CommsRuntime, Config, CoreCommsConfig, ToolError, ToolGatewayBuilder};
use meerkat_core::CommsRuntimeMode;
use meerkat_core::{AgentEvent, format_verbose_event};
use meerkat_tools::builtin::comms::CommsToolSurface;
use meerkat_tools::builtin::shell::ShellConfig;
use meerkat_tools::{
    BuiltinDispatcherConfig, BuiltinToolConfig, CompositeDispatcherError, FileTaskStore,
    MemoryTaskStore, build_builtin_dispatcher, ensure_rkat_dir, find_project_root,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Create a tool dispatcher with built-in tools enabled.
///
/// This is a convenience function for setting up an agent with Meerkat's
/// built-in task management tools. It automatically:
/// - Sets up an in-memory task store (non-persistent)
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
    let store = Arc::new(MemoryTaskStore::new());
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

/// Create a tool dispatcher with built-in tools and a file-backed task store.
///
/// This persists tasks to the provided path (explicit persistence).
pub fn create_dispatcher_with_builtins_persisted(
    config: &BuiltinToolConfig,
    shell_config: Option<ShellConfig>,
    external: Option<Arc<dyn AgentToolDispatcher>>,
    session_id: Option<String>,
    task_store_path: impl AsRef<Path>,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    let store = Arc::new(FileTaskStore::new(task_store_path.as_ref().to_path_buf()));
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

/// Create a tool dispatcher with built-ins using the nearest `.rkat` project root.
///
/// This is a convenience for explicit persistence inside the project.
pub fn create_dispatcher_with_builtins_in_project(
    config: &BuiltinToolConfig,
    shell_config: Option<ShellConfig>,
    external: Option<Arc<dyn AgentToolDispatcher>>,
    session_id: Option<String>,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    let cwd = std::env::current_dir().map_err(CompositeDispatcherError::Io)?;
    let project_root =
        find_project_root(&cwd).ok_or_else(|| CompositeDispatcherError::ToolInitFailed {
            name: "project_root".to_string(),
            message: "No .rkat directory found in current or parent directories".to_string(),
        })?;
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

/// Build a comms runtime from the config and base directory.
///
/// - Inproc mode uses an in-memory runtime (no listeners).
/// - TCP/UDS modes require `config.comms.address` to be set.
pub async fn build_comms_runtime_from_config(
    config: &Config,
    base_dir: impl AsRef<Path>,
    comms_name: &str,
) -> Result<CommsRuntime, String> {
    match config.comms.mode {
        CommsRuntimeMode::Inproc => CommsRuntime::inproc_only(comms_name)
            .map_err(|e| format!("Failed to create inproc comms runtime: {}", e)),
        CommsRuntimeMode::Tcp => {
            let address = config
                .comms
                .address
                .as_ref()
                .ok_or_else(|| "comms.address is required when comms.mode = tcp".to_string())?;
            let listen_tcp = address
                .parse()
                .map_err(|e| format!("Invalid comms TCP address '{}': {}", address, e))?;
            let comms = CoreCommsConfig {
                enabled: true,
                name: comms_name.to_string(),
                listen_tcp: Some(listen_tcp),
                ..Default::default()
            };
            let resolved = comms.resolve_paths(base_dir.as_ref());
            let mut runtime = CommsRuntime::new(resolved)
                .map_err(|e| format!("Failed to create comms runtime: {}", e))?;
            runtime
                .start_listeners()
                .await
                .map_err(|e| format!("Failed to start comms listeners: {}", e))?;
            Ok(runtime)
        }
        CommsRuntimeMode::Uds => {
            let address = config
                .comms
                .address
                .as_ref()
                .ok_or_else(|| "comms.address is required when comms.mode = uds".to_string())?;
            let comms = CoreCommsConfig {
                enabled: true,
                name: comms_name.to_string(),
                listen_uds: Some(std::path::PathBuf::from(address)),
                ..Default::default()
            };
            let resolved = comms.resolve_paths(base_dir.as_ref());
            let mut runtime = CommsRuntime::new(resolved)
                .map_err(|e| format!("Failed to create comms runtime: {}", e))?;
            runtime
                .start_listeners()
                .await
                .map_err(|e| format!("Failed to start comms listeners: {}", e))?;
            Ok(runtime)
        }
    }
}

/// Compose a tool dispatcher with comms tools and append usage instructions.
pub fn compose_tools_with_comms(
    base_tools: Arc<dyn AgentToolDispatcher>,
    tool_usage_instructions: String,
    runtime: &CommsRuntime,
) -> Result<(Arc<dyn AgentToolDispatcher>, String), ToolError> {
    let trusted_peers = runtime.trusted_peers_shared();
    let comms_surface = CommsToolSurface::new(runtime.router_arc(), trusted_peers.clone());
    let availability = CommsToolSurface::peer_availability(trusted_peers);

    let gateway = ToolGatewayBuilder::new()
        .add_dispatcher(base_tools)
        .add_dispatcher_with_availability(Arc::new(comms_surface), availability)
        .build()?;

    let mut instructions = tool_usage_instructions;
    if !instructions.is_empty() {
        instructions.push_str("\n\n");
    }
    instructions.push_str(CommsToolSurface::usage_instructions());

    Ok((Arc::new(gateway), instructions))
}

/// Configuration for the SDK event logger helper.
#[derive(Debug, Clone, Copy, Default)]
pub struct EventLoggerConfig {
    pub verbose: bool,
    pub stream: bool,
}

/// Spawn an event logger that mirrors CLI verbose/stream behavior.
pub fn spawn_event_logger(
    mut agent_event_rx: mpsc::Receiver<AgentEvent>,
    config: EventLoggerConfig,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        use std::io::Write;

        while let Some(event) = agent_event_rx.recv().await {
            if config.stream {
                if let AgentEvent::TextDelta { delta } = &event {
                    print!("{}", delta);
                    let _ = std::io::stdout().flush();
                }
            }

            if !config.verbose {
                continue;
            }

            if let Some(line) = format_verbose_event(&event) {
                eprintln!("{}", line);
            }
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
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
        if std::env::var("RUN_TEST_DISPATCHER_INNER").is_ok() {
            let temp_path = std::env::var("TEST_TEMP_PATH").expect("TEST_TEMP_PATH not set");
            let temp_path = std::path::PathBuf::from(temp_path);
            std::env::set_current_dir(&temp_path).expect("set cwd failed");

            // Create dispatcher using the helper
            let result = create_builtins_dispatcher(
                &BuiltinToolConfig::default(),
                Some("test-123".to_string()),
            );

            assert!(result.is_ok());
            let dispatcher = result.unwrap();
            let tools = dispatcher.tools();
            // 4 task tools + 2 utility tools (wait, datetime)
            assert!(tools.iter().any(|t| t.name == "task_create"));
            assert!(tools.iter().any(|t| t.name == "wait"));
            return;
        }

        // Test the helper function (uses tempdir to avoid polluting the workspace)
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        // Create a .rkat directory to mark it as a project
        std::fs::create_dir_all(temp_path.join(".rkat")).unwrap();

        let status = std::process::Command::new(std::env::current_exe().expect("current exe"))
            .arg("test_create_dispatcher_in_project_dir")
            .env("RUN_TEST_DISPATCHER_INNER", "1")
            .env("TEST_TEMP_PATH", temp_path)
            .status()
            .expect("failed to spawn test child process");

        assert!(status.success());
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
