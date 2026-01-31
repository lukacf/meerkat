//! Tool error types for Meerkat.

pub use meerkat_core::error::{ToolError, ToolValidationError};
use thiserror::Error;

/// Errors that can occur during tool dispatch
#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("Validation error: {0}")]
    Validation(#[from] ToolValidationError),
    #[error("Tool error: {0}")]
    Tool(#[from] ToolError),
    #[error("MCP error: {0}")]
    Mcp(#[from] meerkat_mcp::McpError),
    #[error("Timeout: tool execution took longer than {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },
}
