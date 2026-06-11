//! MCP client errors

use crate::external_tool_surface_authority::ExternalToolSurfaceError;
use meerkat_core::handles::DslTransitionError;

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("Connection failed: {reason}")]
    ConnectionFailed { reason: String },

    #[error("Server not found: {0}")]
    ServerNotFound(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Server '{server}' is not accepting new calls (state: {state})")]
    ServerUnavailable { server: String, state: String },

    #[error("Protocol error: {message}")]
    ProtocolError { message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Tool call failed for '{tool}': {reason}")]
    ToolCallFailed { tool: String, reason: String },

    /// The external-tool-surface owner rejected a staged or boundary input.
    #[error(transparent)]
    SurfaceRejected(#[from] ExternalToolSurfaceError),

    /// The session's MCP server-lifecycle DSL mirror rejected a handshake
    /// transition (K14). The rejection is fail-closed: the shell mirror is
    /// left unchanged and the fault propagates typed instead of being
    /// debug-swallowed.
    #[error("MCP lifecycle mirror rejected for server '{server}': {source}")]
    LifecycleMirrorRejected {
        server: String,
        #[source]
        source: DslTransitionError,
    },

    /// The MCP router has been shut down.
    #[error("MCP router has been shut down")]
    RouterShutDown,
}
