//! Composite dispatcher that combines built-in tools with external dispatchers
//!
//! The [`CompositeDispatcher`] provides a unified tool dispatcher that:
//! - Serves built-in tools (task management, shell execution, etc.) from this crate
//! - Delegates to an external dispatcher (MCP router, etc.) for other tools
//! - Applies policy-based filtering to control which tools are enabled
//! - Detects name collisions between built-in and external tools

use super::config::BuiltinToolConfig;
use super::shell::{ShellConfig, ShellToolSet};
use super::store::TaskStore;
use super::sub_agent::SubAgentToolSet;
use super::tools::{
    TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool, task_tools_usage_instructions,
};
use super::utility::{UtilityToolSet, WaitInterrupt};
use super::{BuiltinTool, BuiltinToolError};
use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::{AgentToolDispatcher, ToolDef};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::watch;

/// Errors that can occur when creating or using CompositeDispatcher
#[derive(Debug, thiserror::Error)]
pub enum CompositeDispatcherError {
    /// Tool name collision between built-in and external tools
    #[error("Tool name collision: built-in tool '{0}' conflicts with external tool")]
    NameCollision(String),

    /// Failed to initialize a built-in tool
    #[error("Failed to initialize built-in tool '{name}': {message}")]
    ToolInitFailed { name: String, message: String },

    /// IO error (e.g., creating directories)
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A tool dispatcher that combines built-in tools with an external dispatcher.
///
/// This dispatcher handles "pure tool concerns" - shell, task management, utilities,
/// sub-agents, and MCP tools. It does NOT handle comms tools because comms is
/// infrastructure, not a tool category.
///
/// For agents that need comms, compose this with a CommsToolSurface via ToolGateway.
pub struct CompositeDispatcher {
    builtins: HashMap<String, Arc<dyn BuiltinTool>>,
    external: Option<Arc<dyn AgentToolDispatcher>>,
    tool_defs: Vec<ToolDef>,
    /// Track which tool sets are enabled for usage instructions
    has_task_tools: bool,
    has_shell_tools: bool,
    has_sub_agent_tools: bool,
    has_utility_tools: bool,
    /// Sender to interrupt the wait tool (if configured)
    wait_interrupt_tx: Option<watch::Sender<Option<WaitInterrupt>>>,
}

impl CompositeDispatcher {
    /// Create a new composite dispatcher with built-in task, shell, and utility tools.
    ///
    /// This creates the dispatcher without wait interrupt support. Use
    /// [`new_with_interrupt`] to enable wait interruption.
    ///
    /// Note: This dispatcher does NOT include comms tools. Comms is infrastructure,
    /// not a tool category. Use ToolGateway to compose with CommsToolSurface.
    pub fn new(
        store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
    ) -> Result<Self, CompositeDispatcherError> {
        Self::new_with_interrupt(store, config, shell_config, external, session_id, false)
    }

    /// Create a composite dispatcher with wait interrupt support.
    ///
    /// When `enable_wait_interrupt` is true, the wait tool can be interrupted
    /// by calling [`interrupt_wait`] on this dispatcher.
    ///
    /// Note: This dispatcher does NOT include comms tools. Comms is infrastructure,
    /// not a tool category. Use ToolGateway to compose with CommsToolSurface.
    pub fn new_with_interrupt(
        store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
        enable_wait_interrupt: bool,
    ) -> Result<Self, CompositeDispatcherError> {
        let resolved = config.resolve();
        let mut builtins: HashMap<String, Arc<dyn BuiltinTool>> = HashMap::new();

        // Create task tools with session tracking
        let task_tools: Vec<Arc<dyn BuiltinTool>> = vec![
            Arc::new(TaskCreateTool::with_session_opt(
                store.clone(),
                session_id.clone(),
            )),
            Arc::new(TaskListTool::new(store.clone())),
            Arc::new(TaskGetTool::new(store.clone())),
            Arc::new(TaskUpdateTool::with_session_opt(store, session_id)),
        ];

        for tool in task_tools {
            if resolved.is_enabled(tool.name(), tool.default_enabled()) {
                builtins.insert(tool.name().to_string(), tool);
            }
        }

        // Create and filter shell tools if config provided
        if let Some(shell_cfg) = shell_config {
            let shell_tool_set = ShellToolSet::new(shell_cfg);
            if resolved.is_enabled(
                shell_tool_set.shell.name(),
                shell_tool_set.shell.default_enabled(),
            ) {
                builtins.insert(
                    shell_tool_set.shell.name().to_string(),
                    Arc::new(shell_tool_set.shell),
                );
            }
            if resolved.is_enabled(
                shell_tool_set.job_status.name(),
                shell_tool_set.job_status.default_enabled(),
            ) {
                builtins.insert(
                    shell_tool_set.job_status.name().to_string(),
                    Arc::new(shell_tool_set.job_status),
                );
            }
            if resolved.is_enabled(
                shell_tool_set.jobs_list.name(),
                shell_tool_set.jobs_list.default_enabled(),
            ) {
                builtins.insert(
                    shell_tool_set.jobs_list.name().to_string(),
                    Arc::new(shell_tool_set.jobs_list),
                );
            }
            if resolved.is_enabled(
                shell_tool_set.job_cancel.name(),
                shell_tool_set.job_cancel.default_enabled(),
            ) {
                builtins.insert(
                    shell_tool_set.job_cancel.name().to_string(),
                    Arc::new(shell_tool_set.job_cancel),
                );
            }
        }

        // Create utility tools (enabled by default)
        // Optionally set up interrupt channel for wait tool
        let (wait_interrupt_tx, utility_tool_set) = if enable_wait_interrupt {
            let (tx, rx) = watch::channel(None);
            (Some(tx), UtilityToolSet::with_interrupt(rx))
        } else {
            (None, UtilityToolSet::new())
        };

        if resolved.is_enabled(
            utility_tool_set.wait.name(),
            utility_tool_set.wait.default_enabled(),
        ) {
            builtins.insert(
                utility_tool_set.wait.name().to_string(),
                Arc::new(utility_tool_set.wait),
            );
        }
        if resolved.is_enabled(
            utility_tool_set.datetime.name(),
            utility_tool_set.datetime.default_enabled(),
        ) {
            builtins.insert(
                utility_tool_set.datetime.name().to_string(),
                Arc::new(utility_tool_set.datetime),
            );
        }

        // Check for collisions with external tools
        if let Some(ref ext) = external {
            for ext_tool in ext.tools() {
                if builtins.contains_key(&ext_tool.name) {
                    return Err(CompositeDispatcherError::NameCollision(ext_tool.name));
                }
            }
        }

        let mut tool_defs: Vec<ToolDef> = builtins.values().map(|t| t.def()).collect();
        if let Some(ref ext) = external {
            tool_defs.extend(ext.tools());
        }

        // Track which tool categories are enabled
        let has_task_tools = builtins.contains_key("task_create")
            || builtins.contains_key("task_list")
            || builtins.contains_key("task_get")
            || builtins.contains_key("task_update");

        let has_shell_tools = builtins.contains_key("shell")
            || builtins.contains_key("shell_job_status")
            || builtins.contains_key("shell_jobs")
            || builtins.contains_key("shell_job_cancel");

        let has_utility_tools = builtins.contains_key("wait") || builtins.contains_key("datetime");

        Ok(Self {
            builtins,
            external,
            tool_defs,
            has_task_tools,
            has_shell_tools,
            has_sub_agent_tools: false, // Set later via register_sub_agent_tools
            has_utility_tools,
            wait_interrupt_tx,
        })
    }

    /// Interrupt the wait tool with a reason.
    ///
    /// This will cause any currently running `wait` call to return early
    /// with status "interrupted" and the provided reason.
    ///
    /// Returns `false` if wait interruption was not enabled during construction.
    pub fn interrupt_wait(&self, reason: &str) -> bool {
        if let Some(ref tx) = self.wait_interrupt_tx {
            let _ = tx.send(Some(WaitInterrupt {
                reason: reason.to_string(),
            }));
            true
        } else {
            false
        }
    }

    /// Get a clone of the wait interrupt sender (for external triggering)
    ///
    /// Returns None if wait interruption was not enabled during construction.
    pub fn wait_interrupt_sender(&self) -> Option<watch::Sender<Option<WaitInterrupt>>> {
        self.wait_interrupt_tx.clone()
    }

    /// Register a comms notify to interrupt waits when messages arrive.
    ///
    /// This spawns a background task that waits on the notify and calls
    /// `interrupt_wait("comms message")` when triggered.
    ///
    /// Call this after construction when comms is enabled for the agent.
    /// Returns `false` if wait interruption was not enabled during construction.
    pub fn register_comms_notify(&self, notify: std::sync::Arc<tokio::sync::Notify>) -> bool {
        let Some(ref tx) = self.wait_interrupt_tx else {
            return false;
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            loop {
                notify.notified().await;
                // Exit loop if receiver is dropped (dispatcher was dropped)
                if tx
                    .send(Some(WaitInterrupt {
                        reason: "comms message".to_string(),
                    }))
                    .is_err()
                {
                    break;
                }
            }
        });
        true
    }

    /// Create with just built-in tools (no external dispatcher).
    pub fn builtins_only(
        store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
        session_id: Option<String>,
    ) -> Result<Self, CompositeDispatcherError> {
        Self::new(store, config, shell_config, None, session_id)
    }

    pub fn builtin_count(&self) -> usize {
        self.builtins.len()
    }

    pub fn is_builtin(&self, name: &str) -> bool {
        self.builtins.contains_key(name)
    }

    /// Register sub-agent tools with this dispatcher
    ///
    /// Sub-agent tools require runtime context (parent session, client factory, etc.)
    /// that isn't available at dispatcher construction time. Call this method after
    /// creating the dispatcher to add sub-agent tool support.
    ///
    /// # Arguments
    /// * `tool_set` - The SubAgentToolSet containing all sub-agent tools
    /// * `config` - Tool configuration to determine which tools are enabled
    ///
    /// # Returns
    /// * `Ok(())` - If tools were registered successfully
    /// * `Err(...)` - If there's a name collision with existing tools
    pub fn register_sub_agent_tools(
        &mut self,
        tool_set: SubAgentToolSet,
        config: &BuiltinToolConfig,
    ) -> Result<(), CompositeDispatcherError> {
        let resolved = config.resolve();

        // Check for collisions first
        for name in tool_set.tool_names() {
            if self.builtins.contains_key(name) {
                return Err(CompositeDispatcherError::NameCollision(name.to_string()));
            }
            if let Some(ref ext) = self.external {
                for ext_tool in ext.tools() {
                    if ext_tool.name == name {
                        return Err(CompositeDispatcherError::NameCollision(name.to_string()));
                    }
                }
            }
        }

        // Register spawn tool
        if resolved.is_enabled(tool_set.spawn.name(), tool_set.spawn.default_enabled()) {
            let def = tool_set.spawn.def();
            self.builtins
                .insert(tool_set.spawn.name().to_string(), Arc::new(tool_set.spawn));
            self.tool_defs.push(def);
        }

        // Register fork tool
        if resolved.is_enabled(tool_set.fork.name(), tool_set.fork.default_enabled()) {
            let def = tool_set.fork.def();
            self.builtins
                .insert(tool_set.fork.name().to_string(), Arc::new(tool_set.fork));
            self.tool_defs.push(def);
        }

        // Register status tool
        if resolved.is_enabled(tool_set.status.name(), tool_set.status.default_enabled()) {
            let def = tool_set.status.def();
            self.builtins.insert(
                tool_set.status.name().to_string(),
                Arc::new(tool_set.status),
            );
            self.tool_defs.push(def);
        }

        // Register cancel tool
        if resolved.is_enabled(tool_set.cancel.name(), tool_set.cancel.default_enabled()) {
            let def = tool_set.cancel.def();
            self.builtins.insert(
                tool_set.cancel.name().to_string(),
                Arc::new(tool_set.cancel),
            );
            self.tool_defs.push(def);
        }

        // Register list tool
        if resolved.is_enabled(tool_set.list.name(), tool_set.list.default_enabled()) {
            let def = tool_set.list.def();
            self.builtins
                .insert(tool_set.list.name().to_string(), Arc::new(tool_set.list));
            self.tool_defs.push(def);
        }

        // Check if any sub-agent tools were actually registered
        self.has_sub_agent_tools = self.builtins.contains_key("agent_spawn")
            || self.builtins.contains_key("agent_fork")
            || self.builtins.contains_key("agent_status")
            || self.builtins.contains_key("agent_cancel")
            || self.builtins.contains_key("agent_list");

        Ok(())
    }

    /// Get usage instructions for all enabled built-in tool categories
    ///
    /// Returns a string containing usage guidance for the LLM on how to properly
    /// use the enabled built-in tools. This should be injected into the system prompt.
    ///
    /// Only includes instructions for tool categories that are actually enabled.
    pub fn usage_instructions(&self) -> String {
        let mut sections = Vec::new();

        if self.has_task_tools {
            sections.push(task_tools_usage_instructions());
        }

        if self.has_shell_tools {
            sections.push(ShellToolSet::usage_instructions());
        }

        if self.has_sub_agent_tools {
            sections.push(SubAgentToolSet::usage_instructions());
        }

        if self.has_utility_tools {
            sections.push(UtilityToolSet::usage_instructions());
        }

        if sections.is_empty() {
            return String::new();
        }

        sections.join("\n\n")
    }
}

#[async_trait]
impl AgentToolDispatcher for CompositeDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        self.tool_defs.clone()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        if let Some(tool) = self.builtins.get(name) {
            return tool.call(args.clone()).await.map_err(|e| match e {
                BuiltinToolError::InvalidArgs(msg) => ToolError::invalid_arguments(name, msg),
                BuiltinToolError::ExecutionFailed(msg) => ToolError::execution_failed(msg),
                BuiltinToolError::TaskError(te) => ToolError::execution_failed(te),
            });
        }
        if let Some(ref ext) = self.external {
            return ext.dispatch(name, args).await;
        }
        Err(ToolError::not_found(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::MemoryTaskStore;
    use crate::builtin::config::{EnforcedToolPolicy, ToolMode, ToolPolicyLayer};
    use std::collections::HashSet;

    fn create_test_dispatcher(config: BuiltinToolConfig) -> CompositeDispatcher {
        let store = Arc::new(MemoryTaskStore::new());
        CompositeDispatcher::new(store, &config, None, None, None).unwrap()
    }

    fn create_test_dispatcher_with_shell(
        config: BuiltinToolConfig,
        shell_config: ShellConfig,
    ) -> CompositeDispatcher {
        let store = Arc::new(MemoryTaskStore::new());
        CompositeDispatcher::new(store, &config, Some(shell_config), None, None).unwrap()
    }

    #[test]
    fn test_composite_dispatcher_default_config() {
        let dispatcher = create_test_dispatcher(BuiltinToolConfig::default());
        let tools = dispatcher.tools();
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(tool_names.contains(&"task_create"));
        assert!(tool_names.contains(&"task_get"));
        assert!(tool_names.contains(&"task_list"));
        assert!(tool_names.contains(&"task_update"));
    }

    #[test]
    fn test_composite_dispatcher_deny_all() {
        let config = BuiltinToolConfig {
            policy: ToolPolicyLayer::new().with_mode(ToolMode::DenyAll),
            enforced: EnforcedToolPolicy::default(),
        };
        let dispatcher = create_test_dispatcher(config);
        assert!(dispatcher.tools().is_empty());
    }

    #[test]
    fn test_composite_dispatcher_allow_list() {
        let config = BuiltinToolConfig {
            policy: ToolPolicyLayer::new()
                .with_mode(ToolMode::AllowList)
                .enable_tool("task_list"),
            enforced: EnforcedToolPolicy::default(),
        };
        let dispatcher = create_test_dispatcher(config);
        let tools = dispatcher.tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "task_list");
    }

    #[test]
    fn test_composite_dispatcher_enforced_deny() {
        let config = BuiltinToolConfig {
            policy: ToolPolicyLayer::new().with_mode(ToolMode::AllowAll),
            enforced: EnforcedToolPolicy {
                allow: HashSet::new(),
                deny: ["task_create"].into_iter().map(String::from).collect(),
            },
        };
        let dispatcher = create_test_dispatcher(config);
        let tools = dispatcher.tools();
        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(!tool_names.contains(&"task_create"));
        assert!(tool_names.contains(&"task_list"));
    }

    #[tokio::test]
    async fn test_composite_dispatcher_dispatch_builtin() {
        let dispatcher = create_test_dispatcher(BuiltinToolConfig::default());
        let args = serde_json::json!({"subject": "Test task", "description": "A test task"});
        let result = dispatcher.dispatch("task_create", &args).await;
        assert!(result.is_ok());
        let task = result.unwrap();
        assert!(task.get("id").is_some());
    }

    #[tokio::test]
    async fn test_composite_dispatcher_dispatch_unknown_tool() {
        let dispatcher = create_test_dispatcher(BuiltinToolConfig::default());
        let result = dispatcher
            .dispatch("unknown_tool", &serde_json::json!({}))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_composite_dispatcher_is_builtin() {
        let dispatcher = create_test_dispatcher(BuiltinToolConfig::default());
        assert!(dispatcher.is_builtin("task_list"));
        assert!(!dispatcher.is_builtin("unknown_tool"));
    }

    #[test]
    fn test_composite_dispatcher_builtin_count() {
        let config = BuiltinToolConfig {
            policy: ToolPolicyLayer::new()
                .with_mode(ToolMode::DenyAll)
                .enable_tool("task_list")
                .enable_tool("task_get"),
            enforced: EnforcedToolPolicy::default(),
        };
        let dispatcher = create_test_dispatcher(config);
        assert_eq!(dispatcher.builtin_count(), 2);
    }

    #[test]
    fn test_builtins_only_convenience() {
        let store = Arc::new(MemoryTaskStore::new());
        let dispatcher = CompositeDispatcher::builtins_only(
            store,
            &BuiltinToolConfig::default(),
            None,
            Some("test-session".to_string()),
        )
        .unwrap();
        // 4 task tools + 2 utility tools = 6
        assert_eq!(dispatcher.builtin_count(), 6);
    }

    mod with_external {
        use super::*;
        use serde_json::json;

        struct MockExternalDispatcher {
            tools: Vec<ToolDef>,
        }

        impl MockExternalDispatcher {
            fn new() -> Self {
                Self {
                    tools: vec![ToolDef {
                        name: "external_tool".to_string(),
                        description: "An external tool".to_string(),
                        input_schema: json!({"type": "object", "properties": {}}),
                    }],
                }
            }
        }

        #[async_trait]
        impl AgentToolDispatcher for MockExternalDispatcher {
            fn tools(&self) -> Vec<ToolDef> {
                self.tools.clone()
            }
            async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
                if name == "external_tool" {
                    Ok(json!({"result": "external"}))
                } else {
                    Err(ToolError::not_found(name.to_string()))
                }
            }
        }

        #[test]
        fn test_composite_with_external_lists_all_tools() {
            let store = Arc::new(MemoryTaskStore::new());
            let external = Arc::new(MockExternalDispatcher::new());
            let dispatcher = CompositeDispatcher::new(
                store,
                &BuiltinToolConfig::default(),
                None,
                Some(external),
                None,
            )
            .unwrap();
            let tools = dispatcher.tools();
            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert!(tool_names.contains(&"task_create"));
            assert!(tool_names.contains(&"external_tool"));
        }

        #[tokio::test]
        async fn test_composite_dispatches_to_external() {
            let store = Arc::new(MemoryTaskStore::new());
            let external = Arc::new(MockExternalDispatcher::new());
            let dispatcher = CompositeDispatcher::new(
                store,
                &BuiltinToolConfig::default(),
                None,
                Some(external),
                None,
            )
            .unwrap();
            let result = dispatcher.dispatch("external_tool", &json!({})).await;
            assert!(result.is_ok());
        }

        #[test]
        fn test_composite_name_collision_error() {
            let store = Arc::new(MemoryTaskStore::new());
            struct OverlappingExternal;
            #[async_trait]
            impl AgentToolDispatcher for OverlappingExternal {
                fn tools(&self) -> Vec<ToolDef> {
                    vec![ToolDef {
                        name: "task_list".to_string(),
                        description: "External".to_string(),
                        input_schema: json!({"type": "object"}),
                    }]
                }
                async fn dispatch(&self, _name: &str, _args: &Value) -> Result<Value, ToolError> {
                    Ok(json!({"source": "external"}))
                }
            }
            let external = Arc::new(OverlappingExternal);
            let result = CompositeDispatcher::new(
                store,
                &BuiltinToolConfig::default(),
                None,
                Some(external),
                None,
            );
            assert!(matches!(
                result,
                Err(CompositeDispatcherError::NameCollision(_))
            ));
        }

        #[test]
        fn test_composite_builtin_takes_precedence_when_disabled() {
            let store = Arc::new(MemoryTaskStore::new());
            let config = BuiltinToolConfig {
                policy: ToolPolicyLayer::new().disable_tool("task_list"),
                enforced: EnforcedToolPolicy::default(),
            };
            struct OverlappingExternal;
            #[async_trait]
            impl AgentToolDispatcher for OverlappingExternal {
                fn tools(&self) -> Vec<ToolDef> {
                    vec![ToolDef {
                        name: "task_list".to_string(),
                        description: "External".to_string(),
                        input_schema: json!({"type": "object"}),
                    }]
                }
                async fn dispatch(&self, _name: &str, _args: &Value) -> Result<Value, ToolError> {
                    Ok(json!({"source": "external"}))
                }
            }
            let external = Arc::new(OverlappingExternal);
            let result = CompositeDispatcher::new(store, &config, None, Some(external), None);
            assert!(result.is_ok());
            assert!(!result.unwrap().is_builtin("task_list"));
        }
    }

    mod dispatch_tests {
        use super::*;
        use serde_json::json;

        #[tokio::test]
        async fn test_composite_dispatch_builtin_invalid_args() {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher = CompositeDispatcher::builtins_only(
                store,
                &BuiltinToolConfig::default(),
                None,
                None,
            )
            .unwrap();
            let result = dispatcher
                .dispatch("task_create", &json!({"subject": "Test"}))
                .await;
            assert!(result.is_err());
        }

        #[tokio::test]
        async fn test_composite_dispatch_not_found() {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher = CompositeDispatcher::builtins_only(
                store,
                &BuiltinToolConfig::default(),
                None,
                None,
            )
            .unwrap();
            let result = dispatcher.dispatch("nonexistent_tool", &json!({})).await;
            assert!(matches!(result.unwrap_err(), ToolError::NotFound { .. }));
        }

        #[tokio::test]
        async fn test_composite_with_session_tracks_creates() {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher = CompositeDispatcher::builtins_only(
                store.clone(),
                &BuiltinToolConfig::default(),
                None,
                Some("test-session-123".to_string()),
            )
            .unwrap();
            let result = dispatcher
                .dispatch(
                    "task_create",
                    &json!({"subject": "Session test", "description": "Task with session"}),
                )
                .await
                .unwrap();
            assert_eq!(result["created_by_session"], "test-session-123");
        }

        #[tokio::test]
        async fn test_composite_with_session_tracks_updates() {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher = CompositeDispatcher::builtins_only(
                store.clone(),
                &BuiltinToolConfig::default(),
                None,
                Some("update-session".to_string()),
            )
            .unwrap();
            let create_result = dispatcher
                .dispatch(
                    "task_create",
                    &json!({"subject": "Task to update", "description": "Will be updated"}),
                )
                .await
                .unwrap();
            let task_id = create_result["id"].as_str().unwrap();
            let update_result = dispatcher
                .dispatch(
                    "task_update",
                    &json!({"id": task_id, "subject": "Updated task"}),
                )
                .await
                .unwrap();
            assert_eq!(update_result["updated_by_session"], "update-session");
        }
    }

    mod shell_tools_tests {
        use super::*;

        fn create_shell_config() -> ShellConfig {
            ShellConfig::with_project_root(std::env::temp_dir())
        }

        fn config_with_shell_enabled() -> BuiltinToolConfig {
            BuiltinToolConfig {
                policy: ToolPolicyLayer::new()
                    .enable_tool("shell")
                    .enable_tool("shell_job_status")
                    .enable_tool("shell_jobs")
                    .enable_tool("shell_job_cancel"),
                enforced: EnforcedToolPolicy::default(),
            }
        }

        #[test]
        fn test_shell_tools_appear_when_explicitly_enabled() {
            let dispatcher = create_test_dispatcher_with_shell(
                config_with_shell_enabled(),
                create_shell_config(),
            );
            let tools = dispatcher.tools();
            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert!(tool_names.contains(&"shell"));
            assert!(tool_names.contains(&"shell_job_status"));
            // 4 task + 4 shell + 2 utility = 10
            assert_eq!(dispatcher.builtin_count(), 10);
        }

        #[test]
        fn test_shell_tools_not_present_without_config() {
            let dispatcher = create_test_dispatcher(BuiltinToolConfig::default());
            let tools = dispatcher.tools();
            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert!(!tool_names.contains(&"shell"));
            // 4 task + 2 utility = 6
            assert_eq!(dispatcher.builtin_count(), 6);
        }

        #[test]
        fn test_shell_tools_disabled_by_default_when_not_explicitly_enabled() {
            let dispatcher = create_test_dispatcher_with_shell(
                BuiltinToolConfig::default(),
                create_shell_config(),
            );
            let tools = dispatcher.tools();
            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert!(!tool_names.contains(&"shell"));
            // 4 task + 2 utility = 6
            assert_eq!(dispatcher.builtin_count(), 6);
        }

        #[test]
        fn test_shell_tools_filtered_by_deny_all() {
            let config = BuiltinToolConfig {
                policy: ToolPolicyLayer::new().with_mode(ToolMode::DenyAll),
                enforced: EnforcedToolPolicy::default(),
            };
            let dispatcher = create_test_dispatcher_with_shell(config, create_shell_config());
            assert!(dispatcher.tools().is_empty());
        }

        #[test]
        fn test_shell_tools_filtered_by_allow_list() {
            let config = BuiltinToolConfig {
                policy: ToolPolicyLayer::new()
                    .with_mode(ToolMode::AllowList)
                    .enable_tool("shell")
                    .enable_tool("task_list"),
                enforced: EnforcedToolPolicy::default(),
            };
            let dispatcher = create_test_dispatcher_with_shell(config, create_shell_config());
            assert_eq!(dispatcher.tools().len(), 2);
        }

        #[test]
        fn test_shell_tools_filtered_by_enforced_deny() {
            let config = BuiltinToolConfig {
                policy: ToolPolicyLayer::new()
                    .enable_tool("shell")
                    .enable_tool("shell_job_status")
                    .enable_tool("shell_jobs")
                    .enable_tool("shell_job_cancel"),
                enforced: EnforcedToolPolicy {
                    allow: HashSet::new(),
                    deny: ["shell", "shell_job_cancel"]
                        .into_iter()
                        .map(String::from)
                        .collect(),
                },
            };
            let dispatcher = create_test_dispatcher_with_shell(config, create_shell_config());
            let tools = dispatcher.tools();
            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert!(!tool_names.contains(&"shell"));
            assert!(tool_names.contains(&"shell_job_status"));
        }

        #[test]
        fn test_shell_tool_is_builtin() {
            let dispatcher = create_test_dispatcher_with_shell(
                config_with_shell_enabled(),
                create_shell_config(),
            );
            assert!(dispatcher.is_builtin("shell"));
        }

        #[test]
        fn test_individual_shell_tool_can_be_disabled() {
            let config = BuiltinToolConfig {
                policy: ToolPolicyLayer::new()
                    .enable_tool("shell")
                    .enable_tool("shell_job_status")
                    .enable_tool("shell_jobs"),
                enforced: EnforcedToolPolicy::default(),
            };
            let dispatcher = create_test_dispatcher_with_shell(config, create_shell_config());
            let tools = dispatcher.tools();
            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert!(!tool_names.contains(&"shell_job_cancel"));
            assert!(tool_names.contains(&"shell"));
        }

        #[test]
        fn test_builtins_only_with_shell_requires_explicit_enable() {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher = CompositeDispatcher::builtins_only(
                store,
                &BuiltinToolConfig::default(),
                Some(create_shell_config()),
                None,
            )
            .unwrap();
            assert_eq!(dispatcher.builtin_count(), 6); // 4 task + 2 utility
            assert!(!dispatcher.is_builtin("shell"));
        }

        #[test]
        fn test_builtins_only_with_shell_enabled() {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher = CompositeDispatcher::builtins_only(
                store,
                &config_with_shell_enabled(),
                Some(create_shell_config()),
                None,
            )
            .unwrap();
            assert_eq!(dispatcher.builtin_count(), 10); // 4 task + 2 utility + 4 shell
            assert!(dispatcher.is_builtin("shell"));
        }
    }

    mod sub_agent_tools_tests {
        use super::*;
        use crate::builtin::sub_agent::{SubAgentConfig, SubAgentToolSet, SubAgentToolState};
        use meerkat_client::{FactoryError, LlmClient, LlmClientFactory, LlmProvider};
        use meerkat_core::AgentSessionStore;
        use meerkat_core::error::AgentError;
        use meerkat_core::ops::ConcurrencyLimits;
        use meerkat_core::session::Session;
        use meerkat_core::sub_agent::SubAgentManager;
        use serde_json::json;
        use tokio::sync::RwLock;

        struct MockClientFactory;

        impl LlmClientFactory for MockClientFactory {
            fn create_client(
                &self,
                _provider: LlmProvider,
                _api_key: Option<String>,
            ) -> Result<Arc<dyn LlmClient>, FactoryError> {
                Err(FactoryError::MissingApiKey("mock".into()))
            }

            fn supported_providers(&self) -> Vec<LlmProvider> {
                vec![]
            }
        }

        struct MockToolDispatcher;

        #[async_trait]
        impl AgentToolDispatcher for MockToolDispatcher {
            fn tools(&self) -> Vec<ToolDef> {
                vec![]
            }

            async fn dispatch(&self, _name: &str, _args: &Value) -> Result<Value, ToolError> {
                Err(ToolError::not_found("mock"))
            }
        }

        struct MockSessionStore;

        #[async_trait]
        impl AgentSessionStore for MockSessionStore {
            async fn save(&self, _session: &Session) -> Result<(), AgentError> {
                Ok(())
            }

            async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
                Ok(None)
            }
        }

        fn create_sub_agent_tool_set() -> SubAgentToolSet {
            let limits = ConcurrencyLimits::default();
            let manager = Arc::new(SubAgentManager::new(limits.clone(), 0));
            let client_factory = Arc::new(MockClientFactory);
            let tool_dispatcher = Arc::new(MockToolDispatcher);
            let session_store = Arc::new(MockSessionStore);
            let parent_session = Arc::new(RwLock::new(Session::new()));
            let config = SubAgentConfig::default();

            let state = Arc::new(SubAgentToolState::new(
                manager,
                client_factory,
                tool_dispatcher,
                session_store,
                parent_session,
                config,
                0,
            ));

            SubAgentToolSet::new(state)
        }

        fn config_with_sub_agent_enabled() -> BuiltinToolConfig {
            BuiltinToolConfig {
                policy: ToolPolicyLayer::new()
                    .enable_tool("agent_spawn")
                    .enable_tool("agent_fork")
                    .enable_tool("agent_status")
                    .enable_tool("agent_cancel")
                    .enable_tool("agent_list"),
                enforced: EnforcedToolPolicy::default(),
            }
        }

        #[test]
        fn test_register_sub_agent_tools() {
            let store = Arc::new(MemoryTaskStore::new());
            let mut dispatcher = CompositeDispatcher::builtins_only(
                store,
                &BuiltinToolConfig::default(),
                None,
                None,
            )
            .unwrap();

            let initial_count = dispatcher.builtin_count();
            assert_eq!(initial_count, 6); // 4 task + 2 utility tools

            let tool_set = create_sub_agent_tool_set();
            dispatcher
                .register_sub_agent_tools(tool_set, &config_with_sub_agent_enabled())
                .unwrap();

            // Should have 4 task + 2 utility + 5 sub-agent tools = 11
            assert_eq!(dispatcher.builtin_count(), 11);
            assert!(dispatcher.is_builtin("agent_spawn"));
            assert!(dispatcher.is_builtin("agent_fork"));
            assert!(dispatcher.is_builtin("agent_status"));
            assert!(dispatcher.is_builtin("agent_cancel"));
            assert!(dispatcher.is_builtin("agent_list"));
        }

        #[test]
        fn test_register_sub_agent_tools_respects_policy() {
            let store = Arc::new(MemoryTaskStore::new());
            let mut dispatcher = CompositeDispatcher::builtins_only(
                store,
                &BuiltinToolConfig::default(),
                None,
                None,
            )
            .unwrap();

            // Only enable agent_spawn and agent_status
            let partial_config = BuiltinToolConfig {
                policy: ToolPolicyLayer::new()
                    .enable_tool("agent_spawn")
                    .enable_tool("agent_status"),
                enforced: EnforcedToolPolicy::default(),
            };

            let tool_set = create_sub_agent_tool_set();
            dispatcher
                .register_sub_agent_tools(tool_set, &partial_config)
                .unwrap();

            // Should have 4 task + 2 utility + 2 sub-agent tools = 8
            assert_eq!(dispatcher.builtin_count(), 8);
            assert!(dispatcher.is_builtin("agent_spawn"));
            assert!(dispatcher.is_builtin("agent_status"));
            assert!(!dispatcher.is_builtin("agent_fork"));
        }

        #[test]
        fn test_sub_agent_tools_disabled_by_default() {
            let store = Arc::new(MemoryTaskStore::new());
            let mut dispatcher = CompositeDispatcher::builtins_only(
                store,
                &BuiltinToolConfig::default(),
                None,
                None,
            )
            .unwrap();

            let tool_set = create_sub_agent_tool_set();
            // Use default config - sub-agent tools should NOT be enabled
            dispatcher
                .register_sub_agent_tools(tool_set, &BuiltinToolConfig::default())
                .unwrap();

            // Should still have just 4 task + 2 utility tools (sub-agent tools not enabled)
            assert_eq!(dispatcher.builtin_count(), 6);
            assert!(!dispatcher.is_builtin("agent_spawn"));
        }

        #[test]
        fn test_sub_agent_tools_in_tool_defs() {
            let store = Arc::new(MemoryTaskStore::new());
            let mut dispatcher = CompositeDispatcher::builtins_only(
                store,
                &BuiltinToolConfig::default(),
                None,
                None,
            )
            .unwrap();

            let tool_set = create_sub_agent_tool_set();
            dispatcher
                .register_sub_agent_tools(tool_set, &config_with_sub_agent_enabled())
                .unwrap();

            let tool_defs = dispatcher.tools();
            let tool_names: Vec<_> = tool_defs.iter().map(|t| t.name.as_str()).collect();
            assert!(tool_names.contains(&"agent_spawn"));
            assert!(tool_names.contains(&"agent_fork"));
        }

        #[test]
        fn test_sub_agent_tools_collision_detection() {
            let store = Arc::new(MemoryTaskStore::new());

            // Create a dispatcher with an external tool named "agent_spawn"
            struct OverlappingExternal;
            #[async_trait]
            impl AgentToolDispatcher for OverlappingExternal {
                fn tools(&self) -> Vec<ToolDef> {
                    vec![ToolDef {
                        name: "agent_spawn".to_string(),
                        description: "Conflicting tool".to_string(),
                        input_schema: json!({"type": "object"}),
                    }]
                }
                async fn dispatch(&self, _name: &str, _args: &Value) -> Result<Value, ToolError> {
                    Ok(json!({}))
                }
            }

            let mut dispatcher = CompositeDispatcher::new(
                store,
                &BuiltinToolConfig::default(),
                None,
                Some(Arc::new(OverlappingExternal)),
                None,
            )
            .unwrap();

            let tool_set = create_sub_agent_tool_set();
            let result =
                dispatcher.register_sub_agent_tools(tool_set, &config_with_sub_agent_enabled());

            assert!(matches!(
                result,
                Err(CompositeDispatcherError::NameCollision(_))
            ));
        }
    }
}
