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
#[cfg(not(target_arch = "wasm32"))]
pub mod file_store;
pub mod memory_store;
#[cfg(not(target_arch = "wasm32"))]
pub mod project;
#[cfg(not(target_arch = "wasm32"))]
pub mod shell;
#[cfg(feature = "skills")]
pub mod skills;
pub mod store;
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
#[cfg(not(target_arch = "wasm32"))]
pub use file_store::FileTaskStore;
pub use memory_store::MemoryTaskStore;
#[cfg(not(target_arch = "wasm32"))]
pub use project::{ensure_rkat_dir, ensure_rkat_dir_async, find_project_root};
pub use store::TaskStore;

use async_trait::async_trait;
use meerkat_core::ToolDef;
use meerkat_core::ops::OperationId;
use meerkat_core::types::ContentBlock;
use serde_json::Value;
use std::sync::Arc;

/// Output type for builtin tools, supporting text and multimodal content.
#[derive(Debug, Clone)]
pub enum ToolOutput {
    /// JSON value (existing behavior). Serialized to text for ToolResult.
    Json(serde_json::Value),
    /// Multimodal content blocks (e.g., images from view_image).
    Blocks(Vec<ContentBlock>),
}

impl ToolOutput {
    /// Try to extract the inner JSON value.
    ///
    /// Returns `None` if this is a `Blocks` variant.
    pub fn into_json(self) -> Option<serde_json::Value> {
        match self {
            Self::Json(v) => Some(v),
            Self::Blocks(_) => None,
        }
    }
}

/// Trait for implementing built-in tools
///
/// Built-in tools are tools that are bundled with the Meerkat agent harness
/// rather than being provided by external MCP servers.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
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
    /// * `Ok(ToolOutput)` - The tool's result (JSON or multimodal blocks)
    /// * `Err(BuiltinToolError)` - If execution failed
    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError>;

    /// Async operation IDs started by this tool call, if any.
    ///
    /// Default is none. Tools that start background or delegated work should
    /// override this so the turn machine can own the exact `WaitingForOps`
    /// wait-set for the current turn.
    fn operation_ids_for_output(&self, _output: &ToolOutput) -> Vec<OperationId> {
        Vec::new()
    }
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
