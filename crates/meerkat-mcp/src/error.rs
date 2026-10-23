//! MCP client errors

use crate::external_tool_surface_authority::ExternalToolSurfaceError;
use meerkat_core::handles::DslTransitionError;

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// A connection's client close did not finish within its bound.
    #[error("MCP server '{server}' close did not finish within {bound:?}")]
    CloseTimedOut {
        server: String,
        bound: std::time::Duration,
    },
    /// The governed attachment control refused this server, at staging or at
    /// the connection's physical entry. Nothing was spawned or contacted.
    #[error("MCP attachment refused: {0}")]
    AttachmentRefused(meerkat_core::mcp_attachment::McpAttachmentRefusal),
    /// Mechanical local-process isolation refused before the MCP target ran.
    /// This never grants or denies caller authority and carries no host secrets.
    #[error(transparent)]
    Confinement(#[from] meerkat_core::confinement::ConfinementRefusal),
    /// A trusted host refused preparation; never carries host payload text.
    #[error(transparent)]
    CallContext(#[from] crate::McpCallContextError),
    /// Preserve selected-account refusal without turning it into an
    /// interactive retry or losing its typed cause in a connection string.
    #[error(transparent)]
    OAuthAccountRejected(meerkat_auth_core::McpOAuthError),
    /// The server is waiting for a human to authorize it through the host's
    /// browser channel. This is typed host status: it carries the target and
    /// never an authorize URL, state or code.
    #[error("MCP server '{}' is awaiting human authorization", .target.server_name())]
    AuthorizationRequired {
        target: Box<meerkat_auth_core::McpServerIdentity>,
    },
    #[error("Connection failed: {reason}")]
    ConnectionFailed { reason: String },

    /// The host's required HTTP egress profile refused this server's
    /// destination (its scheme, host or port, a resolved or literal address,
    /// a redirect hop, or an SSE message endpoint). Nothing was sent to it.
    #[error("MCP server '{server}' HTTP egress refused: {refusal}")]
    EgressRefused {
        server: String,
        refusal: meerkat_auth_core::http_egress::HttpEgressRefusal,
    },

    /// The host's required HTTP egress profile cannot be enforced for this
    /// server (an invalid policy, an unbuildable client, or a credential
    /// owner not bound to the same profile). Nothing was sent; nothing fell
    /// back to a trusted-host client.
    #[error("MCP server '{server}' HTTP egress enforcement unavailable: {reason}")]
    EgressUnavailable {
        server: String,
        reason: meerkat_auth_core::http_egress::HttpEgressUnavailable,
    },

    /// The server dropped the connection's session (HTTP 404) while a
    /// `tools/call` was in flight. The call was not re-sent and the session
    /// was not re-initialized, so whether it took effect is unknown: neither
    /// success nor denial. The connection is dead; later calls are refused unsent until
    /// the server is reconnected.
    #[error(
        "MCP session of server '{server}' expired during tool call '{tool}'; its outcome is uncertain and the server must be reconnected"
    )]
    SessionExpired { server: String, tool: String },

    /// The `tools/call` was answered with a redirect (followed within its
    /// origin, so possibly more than one physical request, or stopped by the
    /// redirect policy) and then failed. Whether any hop took effect is
    /// unknown: neither success nor denial. It was not re-sent.
    #[error(
        "MCP tool call '{tool}' to server '{server}' was redirected and then failed ({reason}); its outcome is uncertain"
    )]
    RedirectedOutcomeUncertain {
        server: String,
        tool: String,
        reason: String,
    },

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

    /// The server's host access policy does not admit this operation. Refused
    /// before anything was sent.
    #[error(
        "MCP server '{server}' does not permit operation '{operation}' under its access policy"
    )]
    OperationNotPermitted { server: String, operation: String },

    /// The server's configured access policy is invalid. No lifecycle or
    /// transport effect has been admitted for it.
    #[error(transparent)]
    InvalidAccessPolicy(#[from] meerkat_core::McpServerAccessError),

    /// The server's `secret_env` declaration is invalid. No lifecycle or
    /// transport effect has been admitted for it.
    #[error(transparent)]
    InvalidSecretEnv(#[from] meerkat_core::McpSecretEnvDeclarationError),

    /// The server's declared service secrets were not resolved for its
    /// launch. Its process was never spawned, and no value was taken from
    /// the host environment instead.
    #[error("MCP server '{server}' was not launched: {refusal}")]
    ServiceSecretRefused {
        server: String,
        refusal: meerkat_core::auth::McpServiceSecretRefusal,
    },

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

    /// The native entry check at the physical handoff refused this call
    /// (current authorization or required operation review). The typed tool
    /// error is carried unchanged back to the tool surface; nothing was sent.
    #[error(transparent)]
    EntryRefused(Box<meerkat_core::ToolError>),

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

    /// The server's working directory ([`meerkat_core::mcp_config::McpStdioConfig::cwd`])
    /// was refused: by its configuration, before any lifecycle or transport
    /// effect, or at launch, before its process existed. Nothing was
    /// spawned, and the launch never fell back to another directory.
    #[error(transparent)]
    WorkingDirectory(#[from] meerkat_core::McpStdioCwdError),
}

impl McpError {
    /// The typed MCP outcome of a required egress profile that did not admit
    /// an effect for `server`.
    pub fn from_egress(
        server: &str,
        error: meerkat_auth_core::http_egress::HttpEgressError,
    ) -> Self {
        use meerkat_auth_core::http_egress::HttpEgressError;
        match error {
            HttpEgressError::Refused(refusal) => Self::EgressRefused {
                server: server.to_owned(),
                refusal,
            },
            HttpEgressError::Unavailable(reason) => Self::EgressUnavailable {
                server: server.to_owned(),
                reason,
            },
        }
    }

    /// The egress outcome this error carries, if it is one.
    pub fn egress_error(&self) -> Option<meerkat_auth_core::http_egress::HttpEgressError> {
        use meerkat_auth_core::http_egress::HttpEgressError;
        match self {
            Self::EgressRefused { refusal, .. } => Some(HttpEgressError::Refused(*refusal)),
            Self::EgressUnavailable { reason, .. } => Some(HttpEgressError::Unavailable(*reason)),
            _ => None,
        }
    }
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
