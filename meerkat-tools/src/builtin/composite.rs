//! Composite dispatcher that combines built-in tools with external dispatchers
//!
//! The [`CompositeDispatcher`] provides a unified tool dispatcher that:
//! - Serves built-in tools (task management, etc.) from this crate
//! - Delegates to an external dispatcher (MCP router, etc.) for other tools
//! - Applies policy-based filtering to control which tools are enabled
//! - Detects name collisions between built-in and external tools

use super::config::BuiltinToolConfig;
use super::store::TaskStore;
use super::tools::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
use super::{BuiltinTool, BuiltinToolError};
use async_trait::async_trait;
use meerkat_core::error::ToolError;
use meerkat_core::{AgentToolDispatcher, ToolDef};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

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
/// Built-in tools are dispatched locally, while unknown tools are forwarded
/// to the external dispatcher (if provided). Built-in tools take precedence
/// over external tools with the same name.
///
/// **Name collisions** between enabled built-in tools and external tools are
/// detected at construction time and result in a `NameCollision` error.
///
/// # Example
///
/// ```ignore
/// use meerkat_tools::builtin::{
///     CompositeDispatcher, BuiltinToolConfig, FileTaskStore,
///     find_project_root, ensure_rkat_dir,
/// };
/// use std::sync::Arc;
///
/// // Set up store and config
/// let project_root = find_project_root(&std::env::current_dir().unwrap());
/// ensure_rkat_dir(&project_root).unwrap();
/// let store = Arc::new(FileTaskStore::in_project(&project_root));
/// let config = BuiltinToolConfig::default();
///
/// // Create dispatcher with no external tools
/// let dispatcher = CompositeDispatcher::new(store, &config, None, None).unwrap();
///
/// // Or create with just built-in tools using the convenience method
/// let dispatcher = CompositeDispatcher::builtins_only(store, &config, None).unwrap();
///
/// // Use with agent
/// let agent = AgentBuilder::new()
///     .model("claude-sonnet-4")
///     .build(llm_client, Arc::new(dispatcher), session_store);
/// ```
pub struct CompositeDispatcher {
    /// Built-in tools indexed by name
    builtins: HashMap<String, Arc<dyn BuiltinTool>>,
    /// External dispatcher for non-builtin tools (optional)
    external: Option<Arc<dyn AgentToolDispatcher>>,
    /// Combined tool definitions from builtins and external
    tool_defs: Vec<ToolDef>,
}

impl CompositeDispatcher {
    /// Create a new composite dispatcher with built-in task tools.
    ///
    /// # Arguments
    /// * `store` - Task store for built-in task tools
    /// * `config` - Configuration for enabling/disabling built-in tools
    /// * `external` - Optional external dispatcher for non-builtin tools (e.g., MCP router)
    /// * `session_id` - Optional session ID for tracking task operations
    ///
    /// # Errors
    /// Returns `CompositeDispatcherError::NameCollision` if an external tool
    /// has the same name as an enabled built-in tool.
    pub fn new(
        store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
    ) -> Result<Self, CompositeDispatcherError> {
        let resolved = config.resolve();
        let mut builtins: HashMap<String, Arc<dyn BuiltinTool>> = HashMap::new();

        // Create all builtin tools with session tracking
        let all_builtins: Vec<Arc<dyn BuiltinTool>> = vec![
            Arc::new(TaskCreateTool::with_session_opt(
                store.clone(),
                session_id.clone(),
            )),
            Arc::new(TaskListTool::new(store.clone())),
            Arc::new(TaskGetTool::new(store.clone())),
            Arc::new(TaskUpdateTool::with_session_opt(store, session_id)),
        ];

        // Filter based on config
        for tool in all_builtins {
            if resolved.is_enabled(tool.name(), tool.default_enabled()) {
                builtins.insert(tool.name().to_string(), tool);
            }
        }

        // Check for collisions with external tools
        if let Some(ref ext) = external {
            for ext_tool in ext.tools() {
                if builtins.contains_key(&ext_tool.name) {
                    return Err(CompositeDispatcherError::NameCollision(ext_tool.name));
                }
            }
        }

        // Build combined tool definitions
        let mut tool_defs: Vec<ToolDef> = builtins.values().map(|t| t.def()).collect();
        if let Some(ref ext) = external {
            tool_defs.extend(ext.tools());
        }

        Ok(Self {
            builtins,
            external,
            tool_defs,
        })
    }

    /// Create with just built-in tools (no external dispatcher)
    ///
    /// This is a convenience method equivalent to `new(store, config, None, session_id)`.
    pub fn builtins_only(
        store: Arc<dyn TaskStore>,
        config: &BuiltinToolConfig,
        session_id: Option<String>,
    ) -> Result<Self, CompositeDispatcherError> {
        Self::new(store, config, None, session_id)
    }

    /// Get the number of built-in tools currently enabled
    pub fn builtin_count(&self) -> usize {
        self.builtins.len()
    }

    /// Check if a tool name is a built-in tool
    pub fn is_builtin(&self, name: &str) -> bool {
        self.builtins.contains_key(name)
    }
}

#[async_trait]
impl AgentToolDispatcher for CompositeDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        self.tool_defs.clone()
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
        // Check built-in tools first
        if let Some(tool) = self.builtins.get(name) {
            return tool.call(args.clone()).await.map_err(|e| match e {
                BuiltinToolError::InvalidArgs(msg) => ToolError::invalid_arguments(name, msg),
                BuiltinToolError::ExecutionFailed(msg) => ToolError::execution_failed(msg),
                BuiltinToolError::TaskError(te) => ToolError::execution_failed(te),
            });
        }

        // Fall back to external dispatcher
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
        CompositeDispatcher::new(store, &config, None, None).unwrap()
    }

    #[test]
    fn test_composite_dispatcher_default_config() {
        let dispatcher = create_test_dispatcher(BuiltinToolConfig::default());

        // All task tools should be enabled by default
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

        // No tools should be available
        let tools = dispatcher.tools();
        assert!(tools.is_empty());
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
        assert!(tool_names.contains(&"task_get"));
        assert!(tool_names.contains(&"task_update"));
    }

    #[tokio::test]
    async fn test_composite_dispatcher_dispatch_builtin() {
        let dispatcher = create_test_dispatcher(BuiltinToolConfig::default());

        // Create a task
        let args = serde_json::json!({
            "subject": "Test task",
            "description": "A test task"
        });
        let result = dispatcher.dispatch("task_create", &args).await;
        assert!(result.is_ok());

        let task = result.unwrap();
        assert!(task.get("id").is_some());
        assert_eq!(task.get("subject").unwrap(), "Test task");
    }

    #[tokio::test]
    async fn test_composite_dispatcher_dispatch_unknown_tool() {
        let dispatcher = create_test_dispatcher(BuiltinToolConfig::default());

        let result = dispatcher
            .dispatch("unknown_tool", &serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_composite_dispatcher_is_builtin() {
        let dispatcher = create_test_dispatcher(BuiltinToolConfig::default());

        assert!(dispatcher.is_builtin("task_list"));
        assert!(dispatcher.is_builtin("task_create"));
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
            Some("test-session".to_string()),
        )
        .unwrap();

        // Should have all builtin tools
        assert_eq!(dispatcher.builtin_count(), 4);
        assert!(dispatcher.is_builtin("task_create"));
        assert!(dispatcher.is_builtin("task_list"));
    }

    // Test with external dispatcher
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
                        input_schema: json!({
                            "type": "object",
                            "properties": {}
                        }),
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
                Some(external),
                None,
            )
            .unwrap();

            let tools = dispatcher.tools();
            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();

            // Should have both builtin and external tools
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
                Some(external),
                None,
            )
            .unwrap();

            let result = dispatcher.dispatch("external_tool", &json!({})).await;
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), json!({"result": "external"}));
        }

        #[test]
        fn test_composite_name_collision_error() {
            // If both builtin and external have a tool with the same name,
            // we should get a NameCollision error
            let store = Arc::new(MemoryTaskStore::new());

            struct OverlappingExternal;

            #[async_trait]
            impl AgentToolDispatcher for OverlappingExternal {
                fn tools(&self) -> Vec<ToolDef> {
                    vec![ToolDef {
                        name: "task_list".to_string(), // Same as builtin
                        description: "External task_list".to_string(),
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
                Some(external),
                None,
            );

            // Should get a name collision error
            match result {
                Err(CompositeDispatcherError::NameCollision(name)) => {
                    assert_eq!(name, "task_list");
                }
                Err(other) => panic!("Expected NameCollision error, got {:?}", other),
                Ok(_) => panic!("Expected error, got Ok"),
            }
        }

        #[test]
        fn test_composite_builtin_takes_precedence_when_disabled() {
            // When a builtin tool is disabled via config, external tools with
            // the same name should be allowed (no collision)
            let store = Arc::new(MemoryTaskStore::new());

            // Disable task_list in config
            let config = BuiltinToolConfig {
                policy: ToolPolicyLayer::new().disable_tool("task_list"),
                enforced: EnforcedToolPolicy::default(),
            };

            struct OverlappingExternal;

            #[async_trait]
            impl AgentToolDispatcher for OverlappingExternal {
                fn tools(&self) -> Vec<ToolDef> {
                    vec![ToolDef {
                        name: "task_list".to_string(), // Same as disabled builtin
                        description: "External task_list".to_string(),
                        input_schema: json!({"type": "object"}),
                    }]
                }

                async fn dispatch(&self, _name: &str, _args: &Value) -> Result<Value, ToolError> {
                    Ok(json!({"source": "external"}))
                }
            }

            let external = Arc::new(OverlappingExternal);
            let result = CompositeDispatcher::new(store, &config, Some(external), None);

            // Should NOT get an error because task_list is disabled in builtins
            assert!(result.is_ok());

            let dispatcher = result.unwrap();
            // task_list should come from external, not builtins
            assert!(!dispatcher.is_builtin("task_list"));

            let tools = dispatcher.tools();
            let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
            assert!(tool_names.contains(&"task_list")); // From external
        }
    }

    // Additional tests for dispatch behavior
    mod dispatch_tests {
        use super::*;
        use serde_json::json;

        #[tokio::test]
        async fn test_composite_dispatch_builtin_invalid_args() {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher =
                CompositeDispatcher::builtins_only(store, &BuiltinToolConfig::default(), None)
                    .unwrap();

            // Missing required 'description' field
            let result = dispatcher
                .dispatch("task_create", &json!({"subject": "Test"}))
                .await;

            assert!(result.is_err());
            let err = result.unwrap_err();
            match err {
                ToolError::InvalidArguments { name, .. } => {
                    assert_eq!(name, "task_create");
                }
                _ => panic!("Expected InvalidArguments error, got {:?}", err),
            }
        }

        #[tokio::test]
        async fn test_composite_dispatch_not_found() {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher =
                CompositeDispatcher::builtins_only(store, &BuiltinToolConfig::default(), None)
                    .unwrap();

            let result = dispatcher.dispatch("nonexistent_tool", &json!({})).await;

            assert!(result.is_err());
            let err = result.unwrap_err();
            match err {
                ToolError::NotFound { name } => {
                    assert_eq!(name, "nonexistent_tool");
                }
                _ => panic!("Expected NotFound error, got {:?}", err),
            }
        }

        #[tokio::test]
        async fn test_composite_with_session_tracks_creates() {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher = CompositeDispatcher::builtins_only(
                store.clone(),
                &BuiltinToolConfig::default(),
                Some("test-session-123".to_string()),
            )
            .unwrap();

            let result = dispatcher
                .dispatch(
                    "task_create",
                    &json!({
                        "subject": "Session test",
                        "description": "Task with session tracking"
                    }),
                )
                .await
                .unwrap();

            assert_eq!(result["created_by_session"], "test-session-123");
            assert_eq!(result["updated_by_session"], "test-session-123");
        }

        #[tokio::test]
        async fn test_composite_with_session_tracks_updates() {
            let store = Arc::new(MemoryTaskStore::new());
            let dispatcher = CompositeDispatcher::builtins_only(
                store.clone(),
                &BuiltinToolConfig::default(),
                Some("update-session".to_string()),
            )
            .unwrap();

            // Create a task
            let create_result = dispatcher
                .dispatch(
                    "task_create",
                    &json!({
                        "subject": "Task to update",
                        "description": "Will be updated"
                    }),
                )
                .await
                .unwrap();

            let task_id = create_result["id"].as_str().unwrap();

            // Update the task
            let update_result = dispatcher
                .dispatch(
                    "task_update",
                    &json!({
                        "id": task_id,
                        "subject": "Updated task"
                    }),
                )
                .await
                .unwrap();

            assert_eq!(update_result["subject"], "Updated task");
            assert_eq!(update_result["updated_by_session"], "update-session");
        }
    }
}
