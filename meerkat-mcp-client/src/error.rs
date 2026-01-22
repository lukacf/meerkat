//! MCP client errors

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("Connection failed: {reason}")]
    ConnectionFailed { reason: String },

    #[error("Server not found: {0}")]
    ServerNotFound(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Protocol error: {message}")]
    ProtocolError { message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Tool call failed for '{tool}': {reason}")]
    ToolCallFailed { tool: String, reason: String },
}
