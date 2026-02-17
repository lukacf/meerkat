//! SDK helper functions for tool dispatcher setup.

use std::path::Path;
use std::sync::Arc;
use std::{cmp, collections::HashSet};

use crate::{AgentFactory, AgentToolDispatcher, Config, HookEngine, HooksConfig};
#[cfg(feature = "comms")]
use crate::{CommsRuntime, CoreCommsConfig, ToolError, ToolGatewayBuilder};
#[cfg(feature = "comms")]
use meerkat_core::CommsRuntimeMode;
use meerkat_core::{AgentEvent, format_verbose_event};
use meerkat_hooks::DefaultHookEngine;
#[cfg(feature = "comms")]
use meerkat_tools::builtin::comms::CommsToolSurface;
use meerkat_tools::builtin::shell::ShellConfig;
use meerkat_tools::{
    BuiltinToolConfig, CompositeDispatcherError, FileTaskStore, MemoryTaskStore, ensure_rkat_dir,
    find_project_root,
};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Resolve layered hooks config (global -> project) without duplicating project entries.
pub async fn resolve_layered_hooks_config(
    context_root: Option<&Path>,
    user_config_root: Option<&Path>,
    active_config: &Config,
) -> HooksConfig {
    let mut user_entries = Vec::new();
    let mut context_entries = Vec::new();

    if let Some(user_root) = user_config_root {
        let user_cfg_path = user_root.join(".rkat").join("config.toml");
        if let Ok(Some(cfg)) = read_hooks_config_from(&user_cfg_path).await {
            user_entries = cfg.entries;
        }
    }

    if let Some(context) = context_root {
        let project_cfg_path = context.join(".rkat").join("config.toml");
        if let Ok(Some(cfg)) = read_hooks_config_from(&project_cfg_path).await {
            context_entries = cfg.entries;
        }
    }

    let active_hooks = &active_config.hooks;
    let mut layered = HooksConfig {
        default_timeout_ms: active_hooks.default_timeout_ms,
        payload_max_bytes: active_hooks.payload_max_bytes,
        background_max_concurrency: cmp::max(1, active_hooks.background_max_concurrency),
        ..HooksConfig::default()
    };

    // Deterministic precedence: active config > context root > user root.
    let mut seen_ids: HashSet<_> = HashSet::new();
    for entry in &active_hooks.entries {
        if seen_ids.insert(entry.id.clone()) {
            layered.entries.push(entry.clone());
        }
    }
    for entry in &context_entries {
        if seen_ids.insert(entry.id.clone()) {
            layered.entries.push(entry.clone());
        }
    }
    for entry in &user_entries {
        if seen_ids.insert(entry.id.clone()) {
            layered.entries.push(entry.clone());
        }
    }

    layered
}

async fn read_hooks_config_from(path: &Path) -> Result<Option<HooksConfig>, std::io::Error> {
    if !tokio::fs::try_exists(path).await? {
        return Ok(None);
    }
    let mut parsed = Config::default();
    let path_buf = path.to_path_buf();
    match parsed.merge_file(&path_buf).await {
        Ok(()) => Ok(Some(parsed.hooks)),
        Err(err) => {
            tracing::warn!(
                "Failed to parse hooks config at {}: {}",
                path.display(),
                err
            );
            Ok(None)
        }
    }
}

/// Build a default hook engine when at least one hook is configured.
pub fn create_default_hook_engine(hooks_config: HooksConfig) -> Option<Arc<dyn HookEngine>> {
    if hooks_config.entries.is_empty() {
        return None;
    }
    Some(Arc::new(DefaultHookEngine::new(hooks_config)))
}

/// Create a tool dispatcher with built-in tools enabled.
///
/// This is a convenience function for setting up an agent with Meerkat's
/// built-in task management tools. It automatically:
/// - Sets up an in-memory task store (non-persistent)
/// - Creates a composite dispatcher with the configured built-in tools
///
/// # Arguments
/// * `factory` - Agent wiring factory used for consistent dispatcher configuration
/// * `config` - Configuration for enabling/disabling built-in tools
/// * `shell_config` - Optional shell tool configuration
/// * `external` - Optional external dispatcher for additional tools (e.g., MCP router)
/// * `session_id` - Optional session ID for tracking tool usage
///
/// # Returns
/// An `Arc<dyn AgentToolDispatcher>` ready to use with `AgentBuilder::build()`
pub async fn create_dispatcher_with_builtins(
    factory: &AgentFactory,
    config: BuiltinToolConfig,
    shell_config: Option<ShellConfig>,
    external: Option<Arc<dyn AgentToolDispatcher>>,
    session_id: Option<String>,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    let store = Arc::new(MemoryTaskStore::new());
    factory
        .build_builtin_dispatcher(store, config, shell_config, external, session_id)
        .await
}

/// Create a tool dispatcher with built-in tools and a file-backed task store.
///
/// This persists tasks to the provided path (explicit persistence).
pub async fn create_dispatcher_with_builtins_persisted(
    factory: &AgentFactory,
    config: BuiltinToolConfig,
    shell_config: Option<ShellConfig>,
    external: Option<Arc<dyn AgentToolDispatcher>>,
    session_id: Option<String>,
    task_store_path: impl AsRef<Path>,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    let store = Arc::new(FileTaskStore::new(task_store_path.as_ref().to_path_buf()));
    factory
        .build_builtin_dispatcher(store, config, shell_config, external, session_id)
        .await
}

/// Create a tool dispatcher with built-ins using the nearest `.rkat` project root.
///
/// This is a convenience for explicit persistence inside the project.
///
/// If `factory.project_root` is set, it is used instead of scanning `cwd`.
pub async fn create_dispatcher_with_builtins_in_project(
    factory: &AgentFactory,
    config: BuiltinToolConfig,
    shell_config: Option<ShellConfig>,
    external: Option<Arc<dyn AgentToolDispatcher>>,
    session_id: Option<String>,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    let project_root_override = factory.project_root.clone();
    let project_root = tokio::task::spawn_blocking(move || {
        if let Some(root) = project_root_override {
            ensure_rkat_dir(&root).map_err(CompositeDispatcherError::Io)?;
            return Ok::<_, CompositeDispatcherError>(root);
        }

        let cwd = std::env::current_dir().map_err(CompositeDispatcherError::Io)?;
        let project_root =
            find_project_root(&cwd).ok_or_else(|| CompositeDispatcherError::ToolInitFailed {
                name: "project_root".to_string(),
                message: "No .rkat directory found in current or parent directories".to_string(),
            })?;
        ensure_rkat_dir(&project_root).map_err(CompositeDispatcherError::Io)?;
        Ok(project_root)
    })
    .await
    .map_err(|e| CompositeDispatcherError::ToolInitFailed {
        name: "project_root".to_string(),
        message: format!("Failed to resolve project root: {}", e),
    })??;

    let store = Arc::new(FileTaskStore::in_project(&project_root));
    factory
        .build_builtin_dispatcher(store, config, shell_config, external, session_id)
        .await
}

/// Create a tool dispatcher with only built-in task tools (no shell tools, no external tools).
///
/// This is a convenience wrapper around [`create_dispatcher_with_builtins`].
pub async fn create_builtins_dispatcher(
    factory: &AgentFactory,
    config: BuiltinToolConfig,
    session_id: Option<String>,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    create_dispatcher_with_builtins(factory, config, None, None, session_id).await
}

/// Create a tool dispatcher with built-in task and shell tools.
pub async fn create_shell_dispatcher(
    factory: &AgentFactory,
    config: BuiltinToolConfig,
    shell_config: ShellConfig,
    session_id: Option<String>,
) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError> {
    create_dispatcher_with_builtins(factory, config, Some(shell_config), None, session_id).await
}

/// Build a comms runtime from the config and base directory.
///
/// - Inproc mode uses an in-memory runtime (no listeners).
/// - TCP/UDS modes require `config.comms.address` to be set.
#[cfg(feature = "comms")]
pub async fn build_comms_runtime_from_config(
    config: &Config,
    base_dir: impl AsRef<Path>,
    comms_name: &str,
    peer_meta: Option<meerkat_core::PeerMeta>,
) -> Result<CommsRuntime, String> {
    build_comms_runtime_from_config_scoped(config, base_dir, comms_name, peer_meta, None).await
}

/// Build a comms runtime from config with optional inproc namespace isolation.
#[cfg(feature = "comms")]
pub async fn build_comms_runtime_from_config_scoped(
    config: &Config,
    base_dir: impl AsRef<Path>,
    comms_name: &str,
    peer_meta: Option<meerkat_core::PeerMeta>,
    inproc_namespace: Option<String>,
) -> Result<CommsRuntime, String> {
    // Parse the optional event listener address (for external plain-text events)
    let event_listen_tcp = config
        .comms
        .event_address
        .as_ref()
        .map(|addr| {
            addr.parse()
                .map_err(|e| format!("Invalid event_address '{}': {}", addr, e))
        })
        .transpose()?;

    let runtime =
        match config.comms.mode {
            CommsRuntimeMode::Inproc => {
                CommsRuntime::inproc_only_scoped(comms_name, inproc_namespace.clone())
                    .map_err(|e| format!("Failed to create inproc comms runtime: {}", e))?
            }
            CommsRuntimeMode::Tcp => {
                let address =
                    config.comms.address.as_ref().ok_or_else(|| {
                        "comms.address is required when comms.mode = tcp".to_string()
                    })?;
                let listen_tcp = address
                    .parse()
                    .map_err(|e| format!("Invalid comms TCP address '{}': {}", address, e))?;
                let comms = CoreCommsConfig {
                    enabled: true,
                    name: comms_name.to_string(),
                    inproc_namespace: inproc_namespace.clone(),
                    listen_tcp: Some(listen_tcp),
                    auth: config.comms.auth,
                    event_listen_tcp,
                    ..Default::default()
                };
                let resolved = comms.resolve_paths(base_dir.as_ref());
                let mut rt = CommsRuntime::new(resolved)
                    .await
                    .map_err(|e| format!("Failed to create comms runtime: {}", e))?;
                rt.start_listeners()
                    .await
                    .map_err(|e| format!("Failed to start comms listeners: {}", e))?;
                rt
            }
            CommsRuntimeMode::Uds => {
                let address =
                    config.comms.address.as_ref().ok_or_else(|| {
                        "comms.address is required when comms.mode = uds".to_string()
                    })?;
                let comms = CoreCommsConfig {
                    enabled: true,
                    name: comms_name.to_string(),
                    inproc_namespace: inproc_namespace.clone(),
                    listen_uds: Some(std::path::PathBuf::from(address)),
                    auth: config.comms.auth,
                    event_listen_tcp,
                    ..Default::default()
                };
                let resolved = comms.resolve_paths(base_dir.as_ref());
                let mut rt = CommsRuntime::new(resolved)
                    .await
                    .map_err(|e| format!("Failed to create comms runtime: {}", e))?;
                rt.start_listeners()
                    .await
                    .map_err(|e| format!("Failed to start comms listeners: {}", e))?;
                rt
            }
        };

    if let Some(meta) = peer_meta {
        runtime.set_peer_meta(meta);
    }

    Ok(runtime)
}

/// Compose a tool dispatcher with comms tools and append usage instructions.
#[cfg(feature = "comms")]
pub fn compose_tools_with_comms(
    base_tools: Arc<dyn AgentToolDispatcher>,
    tool_usage_instructions: String,
    runtime: &CommsRuntime,
) -> Result<(Arc<dyn AgentToolDispatcher>, String), ToolError> {
    let router = runtime.router_arc();
    let trusted_peers = runtime.trusted_peers_shared();
    let self_pubkey = router.keypair_arc().public_key();
    let comms_surface = CommsToolSurface::new(router, trusted_peers.clone());
    let availability = CommsToolSurface::peer_availability(trusted_peers, self_pubkey);

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
            if config.stream
                && let AgentEvent::TextDelta { delta } = &event
            {
                print!("{}", delta);
                let _ = std::io::stdout().flush();
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
    use meerkat_core::ToolCallView;
    use meerkat_core::{
        HookCapability, HookEntryConfig, HookExecutionMode, HookId, HookPoint, HookRuntimeConfig,
    };
    use std::path::Path;

    async fn dispatch_json(
        dispatcher: &dyn AgentToolDispatcher,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, ToolError> {
        let args_raw =
            serde_json::value::RawValue::from_string(args.to_string()).expect("valid args json");
        let call = ToolCallView {
            id: "test-1",
            name,
            args: &args_raw,
        };
        let result = dispatcher.dispatch(call).await?;
        serde_json::from_str(&result.content).or(Ok(serde_json::Value::String(result.content)))
    }

    #[tokio::test]
    async fn test_builtin_tools_dispatch() {
        let temp_dir = tempfile::tempdir().unwrap();
        let factory = AgentFactory::new(temp_dir.path().join("sessions"));
        let dispatcher = create_builtins_dispatcher(&factory, BuiltinToolConfig::default(), None)
            .await
            .unwrap();

        // Create a task
        let args = serde_json::json!({
            "subject": "Integration test task",
            "description": "Testing the builtin dispatcher"
        });
        let result = dispatch_json(dispatcher.as_ref(), "task_create", args).await;
        assert!(result.is_ok());

        let task = result.unwrap();
        assert!(task.get("id").is_some());
        assert_eq!(task.get("subject").unwrap(), "Integration test task");

        // List tasks - returns an array directly
        let list_result =
            dispatch_json(dispatcher.as_ref(), "task_list", serde_json::json!({})).await;
        assert!(list_result.is_ok());
        let list = list_result.unwrap();
        assert!(list.is_array());
        let tasks = list.as_array().unwrap();
        assert_eq!(tasks.len(), 1);
    }

    #[tokio::test]
    async fn test_create_dispatcher_in_project_dir() {
        // Test the helper function (uses tempdir to avoid polluting the workspace).
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path().to_path_buf();

        // Create a .rkat directory to mark it as a project
        std::fs::create_dir_all(temp_path.join(".rkat")).unwrap();

        let factory = AgentFactory::new(temp_path.join(".rkat").join("sessions"))
            .project_root(temp_path.clone());

        let dispatcher = create_dispatcher_with_builtins_in_project(
            &factory,
            BuiltinToolConfig::default(),
            None,
            None,
            Some("test-123".to_string()),
        )
        .await
        .unwrap();

        let tools = dispatcher.tools();
        assert!(tools.iter().any(|t| t.name == "task_create"));
        assert!(tools.iter().any(|t| t.name == "wait"));

        let _ = dispatch_json(
            dispatcher.as_ref(),
            "task_create",
            serde_json::json!({"subject":"Test","description":"Persist"}),
        )
        .await
        .unwrap();

        // Verify tasks.json was created in .rkat directory.
        let tasks_file = temp_path.join(".rkat").join("tasks.json");
        assert!(tasks_file.exists(), "tasks.json should be created");
    }

    #[tokio::test]
    async fn test_builtin_tools_in_project_dir() {
        // Test using FileTaskStore in a temp directory
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        // Create the .rkat directory
        ensure_rkat_dir(temp_path).unwrap();

        let factory = AgentFactory::new(temp_path.join(".rkat").join("sessions"));
        let tasks_file = temp_path.join(".rkat").join("tasks.json");
        let dispatcher = create_dispatcher_with_builtins_persisted(
            &factory,
            BuiltinToolConfig::default(),
            None,
            None,
            Some("file-test-session".to_string()),
            &tasks_file,
        )
        .await
        .unwrap();

        // Create a task
        let create_result = dispatch_json(
            dispatcher.as_ref(),
            "task_create",
            serde_json::json!({
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
        assert!(tasks_file.exists(), "tasks.json should be created");

        // Get the task back
        let get_result = dispatch_json(
            dispatcher.as_ref(),
            "task_get",
            serde_json::json!({"id": task_id}),
        )
        .await;
        assert!(get_result.is_ok());
        let retrieved = get_result.unwrap();
        assert_eq!(retrieved.get("subject").unwrap(), "File store test");
    }

    #[test]
    fn test_create_default_hook_engine_none_when_no_entries() {
        assert!(create_default_hook_engine(HooksConfig::default()).is_none());
    }

    #[test]
    fn test_create_default_hook_engine_some_when_entries_exist() {
        let hooks = HooksConfig {
            entries: vec![HookEntryConfig {
                id: HookId::new("sdk-hook"),
                point: HookPoint::TurnBoundary,
                mode: HookExecutionMode::Foreground,
                capability: HookCapability::Observe,
                runtime: HookRuntimeConfig::new(
                    "in_process",
                    Some(serde_json::json!({"name":"sdk_hook"})),
                )
                .unwrap_or_default(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(create_default_hook_engine(hooks).is_some());
    }

    fn mk_hook(id: &str, command: &str) -> HookEntryConfig {
        HookEntryConfig {
            id: HookId::new(id),
            point: HookPoint::TurnBoundary,
            mode: HookExecutionMode::Foreground,
            capability: HookCapability::Observe,
            runtime: HookRuntimeConfig::new(
                "command",
                Some(serde_json::json!({ "command": command })),
            )
            .unwrap_or_default(),
            ..Default::default()
        }
    }

    async fn write_config_with_hooks(root: &Path, hooks: Vec<HookEntryConfig>) {
        let mut cfg = Config::default();
        cfg.hooks.entries = hooks;
        let payload = toml::to_string(&cfg).expect("serialize config");
        let dir = root.join(".rkat");
        tokio::fs::create_dir_all(&dir)
            .await
            .expect("create .rkat dir");
        tokio::fs::write(dir.join("config.toml"), payload)
            .await
            .expect("write config");
    }

    #[tokio::test]
    async fn resolve_layered_hooks_respects_precedence() {
        let temp = tempfile::tempdir().expect("tempdir");
        let user_root = temp.path().join("user");
        let context_root = temp.path().join("context");
        tokio::fs::create_dir_all(&user_root)
            .await
            .expect("user root");
        tokio::fs::create_dir_all(&context_root)
            .await
            .expect("context root");

        write_config_with_hooks(
            &user_root,
            vec![mk_hook("dup", "echo user"), mk_hook("u", "echo u")],
        )
        .await;
        write_config_with_hooks(
            &context_root,
            vec![mk_hook("dup", "echo context"), mk_hook("c", "echo c")],
        )
        .await;

        let mut active = Config::default();
        active.hooks.entries = vec![mk_hook("dup", "echo active"), mk_hook("a", "echo a")];

        let resolved =
            resolve_layered_hooks_config(Some(&context_root), Some(&user_root), &active).await;
        let ids: Vec<String> = resolved.entries.iter().map(|h| h.id.0.clone()).collect();
        assert_eq!(ids, vec!["dup", "a", "c", "u"]);

        let first_runtime = resolved.entries[0]
            .runtime
            .config_value()
            .expect("runtime config");
        assert_eq!(first_runtime["command"], "echo active");
    }

    #[tokio::test]
    async fn resolve_layered_hooks_without_roots_uses_active_only() {
        let mut active = Config::default();
        active.hooks.entries = vec![mk_hook("only-active", "echo active")];
        let resolved = resolve_layered_hooks_config(None, None, &active).await;
        assert_eq!(resolved.entries.len(), 1);
        assert_eq!(resolved.entries[0].id.0, "only-active");
    }
}
