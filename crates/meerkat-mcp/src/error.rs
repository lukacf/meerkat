//! MCP client errors

use crate::external_tool_surface_authority::ExternalToolSurfaceError;
use meerkat_core::handles::DslTransitionError;

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// Preserve selected-account refusal without turning it into an
    /// interactive retry or losing its typed cause in a connection string.
    #[error(transparent)]
    OAuthAccountRejected(meerkat_auth_core::McpOAuthError),
    #[error("Connection failed: {reason}")]
    ConnectionFailed { reason: String },

    #[error("Server not found: {0}")]
    ServerNotFound(String),

    /// An explicit exposure map is invalid. No lifecycle or transport effect
    /// has been admitted for this requested configuration.
    #[error(
        "Invalid tool name mapping for server '{server}', operation '{raw_operation}': {reason}"
    )]
    InvalidToolNameMapping {
        server: String,
        raw_operation: String,
        reason: &'static str,
    },

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

    /// A `tools/list` pagination cursor the server had already returned came
    /// back again, so following it would never reach the last page. The
    /// partial tool list is refused.
    #[error("Server '{server}' repeated tool discovery pagination cursor '{cursor}'")]
    ToolDiscoveryCursorRepeated { server: String, cursor: String },

    /// Tool discovery reached a declared enumeration bound before the server
    /// reported its last page. The partial tool list is refused.
    #[error("Server '{server}' exceeded the tool discovery bound: {limit}")]
    ToolDiscoveryLimitExceeded {
        server: String,
        limit: ToolDiscoveryLimit,
    },
}

/// The declared tool discovery bound a server exceeded.
///
/// The bounds are [`McpConnection::MAX_TOOL_DISCOVERY_PAGES`] and
/// [`McpConnection::MAX_DISCOVERED_TOOLS`].
///
/// [`McpConnection::MAX_TOOL_DISCOVERY_PAGES`]: crate::McpConnection::MAX_TOOL_DISCOVERY_PAGES
/// [`McpConnection::MAX_DISCOVERED_TOOLS`]: crate::McpConnection::MAX_DISCOVERED_TOOLS
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDiscoveryLimit {
    /// The server still had a next page after `max` pages.
    Pages { max: usize },
    /// The server listed more than `max` tools across its pages.
    Tools { max: usize },
}

impl std::fmt::Display for ToolDiscoveryLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pages { max } => write!(f, "more than {max} tools/list pages"),
            Self::Tools { max } => write!(f, "more than {max} tools"),
        }
    }
}
