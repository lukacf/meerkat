//! Tool errors

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Tool not found: {0}")]
    NotFound(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Execution error: {0}")]
    Execution(String),

    #[error("Timeout: {0}")]
    Timeout(String),
}

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
    Mcp(#[from] raik_mcp_client::McpError),

    #[error("Tool {tool} timed out after {timeout_ms}ms")]
    Timeout { tool: String, timeout_ms: u64 },
}
