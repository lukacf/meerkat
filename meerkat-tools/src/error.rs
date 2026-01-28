//! Tool errors

pub type ToolError = meerkat_core::error::ToolError;

#[derive(Debug, thiserror::Error)]
pub enum ToolValidationError {
    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Schema validation failed: {0}")]
    SchemaValidation(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Invalid type for field {field}: expected {expected}")]
    InvalidType { field: String, expected: String },
}

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("Tool validation error: {0}")]
    Validation(#[from] ToolValidationError),

    #[error("MCP error: {0}")]
    Mcp(#[from] meerkat_mcp_client::McpError),

    #[error("Tool {tool} timed out after {timeout_ms}ms")]
    Timeout { tool: String, timeout_ms: u64 },
}
