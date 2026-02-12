//! Built-in tools for Meerkat
//!
//! This module provides the [`BuiltinTool`] trait for implementing built-in tools,
//! along with the [`BuiltinToolEntry`] wrapper for tracking enabled state.
//!
//! ## Task Management Types
//!
//! The [`types`] module provides core types for task management:
//! - [`types::Task`] - A task in the system
//! - [`types::TaskId`] - UUID-based task identifier
//! - [`types::TaskStatus`] - Task lifecycle states
//! - [`types::TaskPriority`] - Task priority levels
//!
//! The [`store`] module provides the [`store::TaskStore`] trait for task persistence.
//! The [`memory_store`] module provides [`MemoryTaskStore`] for testing.
//! The [`file_store`] module provides [`FileTaskStore`] for persistent storage.

#[cfg(feature = "comms")]
pub mod comms;
pub mod composite;
pub mod config;
pub mod file_store;
pub mod memory_store;
pub mod project;
pub mod shell;
#[cfg(feature = "skills")]
pub mod skills;
pub mod store;
#[cfg(feature = "sub-agents")]
pub mod sub_agent;
pub mod tasks;
pub mod types;
pub mod utility;

// Re-export core types for convenience
#[cfg(feature = "comms")]
pub use comms::CommsToolSurface;
pub use composite::{CompositeDispatcher, CompositeDispatcherError};
pub use config::{
    BuiltinToolConfig, EnforcedToolPolicy, ResolvedToolPolicy, ToolMode, ToolPolicyLayer,
};
pub use file_store::FileTaskStore;
pub use memory_store::MemoryTaskStore;
pub use project::{ensure_rkat_dir, ensure_rkat_dir_async, find_project_root};
pub use store::TaskStore;

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
