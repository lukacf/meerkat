//! Built-in tools for Meerkat agents
//!
//! This module provides the [`BuiltinTool`] trait for implementing built-in tools,
//! along with the [`BuiltinToolEntry`] wrapper for tracking enabled state.
//!
//! ## Task Management Types
//!
//! The [`types`] module provides core types for task management:
//! - [`types::Task`] - A task in the system
//! - [`types::TaskId`] - ULID-based task identifier
//! - [`types::TaskStatus`] - Task lifecycle states
//! - [`types::TaskPriority`] - Task priority levels
//!
//! The [`store`] module provides the [`store::TaskStore`] trait for task persistence.
//! The [`memory_store`] module provides [`MemoryTaskStore`] for testing.
//! The [`file_store`] module provides [`FileTaskStore`] for persistent storage.
//!
//! ## CompositeDispatcher
//!
//! The [`CompositeDispatcher`] combines built-in tools with external dispatchers (e.g., MCP):
//!
//! ```ignore
//! use meerkat_tools::builtin::{
//!     CompositeDispatcher, BuiltinToolConfig, FileTaskStore,
//!     find_project_root, ensure_rkat_dir,
//! };
//!
//! let project_root = find_project_root(&std::env::current_dir().unwrap());
//! ensure_rkat_dir(&project_root).unwrap();
//! let store = Arc::new(FileTaskStore::in_project(&project_root));
//! let dispatcher = CompositeDispatcher::new(store, &BuiltinToolConfig::default(), None, None)?;
//! ```

mod composite;
mod config;
pub mod file_store;
pub mod memory_store;
pub mod project;
pub mod shell;
pub mod store;
pub mod tools;
pub mod types;

pub use composite::{CompositeDispatcher, CompositeDispatcherError};
pub use config::{
    BuiltinToolConfig, EnforcedToolPolicy, ResolvedToolPolicy, ToolMode, ToolPolicyLayer,
};
pub use file_store::FileTaskStore;
pub use memory_store::MemoryTaskStore;
pub use project::{ensure_rkat_dir, find_project_root};
pub use store::TaskStore;
pub use tools::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
pub use types::{
    NewTask, Task, TaskError, TaskId, TaskPriority, TaskStatus, TaskStoreData, TaskStoreMeta,
    TaskUpdate,
};

use async_trait::async_trait;
use meerkat_core::ToolDef;
use serde_json::Value;
use std::sync::Arc;

/// Trait for implementing built-in tools
///
/// Built-in tools are tools that are bundled with the Meerkat agent harness
/// rather than being provided by external MCP servers.
#[async_trait]
pub trait BuiltinTool: Send + Sync {
    /// Returns the unique name of this tool
    fn name(&self) -> &'static str;

    /// Returns the tool definition for the LLM
    fn def(&self) -> ToolDef;

    /// Returns whether this tool is enabled by default
    fn default_enabled(&self) -> bool;

    /// Execute the tool with the given arguments
    ///
    /// # Arguments
    /// * `args` - JSON value containing the tool arguments
    ///
    /// # Returns
    /// * `Ok(Value)` - The tool's result as JSON
    /// * `Err(BuiltinToolError)` - If execution failed
    async fn call(&self, args: Value) -> Result<Value, BuiltinToolError>;
}

/// A registry entry for a built-in tool with its enabled state
#[derive(Clone)]
pub struct BuiltinToolEntry {
    /// The tool implementation
    pub tool: Arc<dyn BuiltinTool>,
    /// Whether the tool is currently enabled
    pub enabled: bool,
}

impl BuiltinToolEntry {
    /// Create a new entry with the tool's default enabled state
    pub fn new(tool: Arc<dyn BuiltinTool>) -> Self {
        let enabled = tool.default_enabled();
        Self { tool, enabled }
    }

    /// Create a new entry with a specific enabled state
    pub fn with_enabled(tool: Arc<dyn BuiltinTool>, enabled: bool) -> Self {
        Self { tool, enabled }
    }
}

/// Errors that can occur during built-in tool execution
#[derive(Debug, thiserror::Error)]
pub enum BuiltinToolError {
    /// Invalid arguments were provided to the tool
    #[error("Invalid arguments: {0}")]
    InvalidArgs(String),

    /// The tool execution failed
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    /// An async task error occurred
    #[error("Task error: {0}")]
    TaskError(String),
}

impl BuiltinToolError {
    /// Create an InvalidArgs error from a message
    pub fn invalid_args(msg: impl Into<String>) -> Self {
        Self::InvalidArgs(msg.into())
    }

    /// Create an ExecutionFailed error from a message
    pub fn execution_failed(msg: impl Into<String>) -> Self {
        Self::ExecutionFailed(msg.into())
    }

    /// Create a TaskError from a message
    pub fn task_error(msg: impl Into<String>) -> Self {
        Self::TaskError(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_tool_error_invalid_args() {
        let err = BuiltinToolError::InvalidArgs("missing field 'path'".to_string());
        assert_eq!(err.to_string(), "Invalid arguments: missing field 'path'");

        let err = BuiltinToolError::invalid_args("missing field 'path'");
        assert_eq!(err.to_string(), "Invalid arguments: missing field 'path'");
    }

    #[test]
    fn test_builtin_tool_error_execution_failed() {
        let err = BuiltinToolError::ExecutionFailed("file not found".to_string());
        assert_eq!(err.to_string(), "Execution failed: file not found");

        let err = BuiltinToolError::execution_failed("file not found");
        assert_eq!(err.to_string(), "Execution failed: file not found");
    }

    #[test]
    fn test_builtin_tool_error_task_error() {
        let err = BuiltinToolError::TaskError("timeout".to_string());
        assert_eq!(err.to_string(), "Task error: timeout");

        let err = BuiltinToolError::task_error("timeout");
        assert_eq!(err.to_string(), "Task error: timeout");
    }

    #[test]
    fn test_builtin_tool_error_debug() {
        let err = BuiltinToolError::InvalidArgs("test".to_string());
        let debug = format!("{:?}", err);
        assert!(debug.contains("InvalidArgs"));
        assert!(debug.contains("test"));
    }

    // Test that BuiltinToolEntry works correctly
    mod entry_tests {
        use super::*;
        use serde_json::json;

        struct MockTool {
            default_enabled: bool,
        }

        #[async_trait]
        impl BuiltinTool for MockTool {
            fn name(&self) -> &'static str {
                "mock_tool"
            }

            fn def(&self) -> ToolDef {
                ToolDef {
                    name: "mock_tool".to_string(),
                    description: "A mock tool for testing".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {}
                    }),
                }
            }

            fn default_enabled(&self) -> bool {
                self.default_enabled
            }

            async fn call(&self, _args: Value) -> Result<Value, BuiltinToolError> {
                Ok(json!({"result": "ok"}))
            }
        }

        #[test]
        fn test_entry_new_default_enabled() {
            let tool = Arc::new(MockTool {
                default_enabled: true,
            });
            let entry = BuiltinToolEntry::new(tool);
            assert!(entry.enabled);
            assert_eq!(entry.tool.name(), "mock_tool");
        }

        #[test]
        fn test_entry_new_default_disabled() {
            let tool = Arc::new(MockTool {
                default_enabled: false,
            });
            let entry = BuiltinToolEntry::new(tool);
            assert!(!entry.enabled);
        }

        #[test]
        fn test_entry_with_enabled_override() {
            let tool = Arc::new(MockTool {
                default_enabled: false,
            });
            let entry = BuiltinToolEntry::with_enabled(tool, true);
            assert!(entry.enabled);
        }

        #[tokio::test]
        async fn test_mock_tool_call() {
            let tool = MockTool {
                default_enabled: true,
            };
            let result = tool.call(json!({})).await.unwrap();
            assert_eq!(result, json!({"result": "ok"}));
        }
    }
}
