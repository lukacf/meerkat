//! Agent errors for Meerkat

use crate::types::SessionId;

/// Error returned by tool dispatch operations.
///
/// This type represents errors that occur during tool execution, distinguishing
/// between tool-level failures (which should be reported back to the LLM) and
/// system-level failures (which may need different handling).
#[derive(Debug, Clone, thiserror::Error)]
pub enum ToolError {
    /// The requested tool was not found
    #[error("Tool not found: {name}")]
    NotFound { name: String },

    /// The tool exists but is currently unavailable
    ///
    /// Unlike NotFound (tool doesn't exist), this indicates the tool exists
    /// but cannot be used right now due to runtime conditions (e.g., no peers
    /// configured for comms tools).
    #[error("Tool '{name}' is currently unavailable: {reason}")]
    Unavailable { name: String, reason: String },

    /// The tool arguments failed validation
    #[error("Invalid arguments for tool '{name}': {reason}")]
    InvalidArguments { name: String, reason: String },

    /// The tool execution failed
    #[error("Tool execution failed: {message}")]
    ExecutionFailed { message: String },

    /// The tool execution timed out
    #[error("Tool '{name}' timed out after {timeout_ms}ms")]
    Timeout { name: String, timeout_ms: u64 },

    /// Tool access was denied by policy
    #[error("Tool '{name}' is not allowed by policy")]
    AccessDenied { name: String },

    /// A generic tool error with a message
    #[error("{0}")]
    Other(String),
}

impl ToolError {
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "tool_not_found",
            Self::Unavailable { .. } => "tool_unavailable",
            Self::InvalidArguments { .. } => "invalid_arguments",
            Self::ExecutionFailed { .. } => "execution_failed",
            Self::Timeout { .. } => "timeout",
            Self::AccessDenied { .. } => "access_denied",
            Self::Other(_) => "tool_error",
        }
    }

    pub fn to_error_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "error": self.error_code(),
            "message": self.to_string(),
        })
    }

    /// Create a new "not found" error
    pub fn not_found(name: impl Into<String>) -> Self {
        Self::NotFound { name: name.into() }
    }

    /// Create a new "unavailable" error
    pub fn unavailable(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Unavailable {
            name: name.into(),
            reason: reason.into(),
        }
    }

    /// Create a new "invalid arguments" error
    pub fn invalid_arguments(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidArguments {
            name: name.into(),
            reason: reason.into(),
        }
    }

    /// Create a new "execution failed" error
    pub fn execution_failed(message: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: message.into(),
        }
    }

    /// Create a new "timeout" error
    pub fn timeout(name: impl Into<String>, timeout_ms: u64) -> Self {
        Self::Timeout {
            name: name.into(),
            timeout_ms,
        }
    }

    /// Create a new "access denied" error
    pub fn access_denied(name: impl Into<String>) -> Self {
        Self::AccessDenied { name: name.into() }
    }

    /// Create a generic error from any string-like type
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }
}

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

impl From<&str> for ToolError {
    fn from(s: &str) -> Self {
        Self::Other(s.to_string())
    }
}

/// Errors that can occur during agent execution
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("LLM error: {0}")]
    LlmError(String),

    #[error("Storage error: {0}")]
    StoreError(String),

    #[error("Tool error: {0}")]
    ToolError(String),

    #[error("MCP error: {0}")]
    McpError(String),

    #[error("Session not found: {0}")]
    SessionNotFound(SessionId),

    #[error("Token budget exceeded: used {used}, limit {limit}")]
    TokenBudgetExceeded { used: u64, limit: u64 },

    #[error("Time budget exceeded: {elapsed_secs}s > {limit_secs}s")]
    TimeBudgetExceeded { elapsed_secs: u64, limit_secs: u64 },

    #[error("Tool call budget exceeded: {count} calls > {limit} limit")]
    ToolCallBudgetExceeded { count: usize, limit: usize },

    #[error("Max tokens reached on turn {turn}, partial output: {partial}")]
    MaxTokensReached { turn: u32, partial: String },

    #[error("Content filtered on turn {turn}")]
    ContentFiltered { turn: u32 },

    #[error("Max turns reached: {turns}")]
    MaxTurnsReached { turns: u32 },

    #[error("Run was cancelled")]
    Cancelled,

    #[error("Invalid state transition: {from} -> {to}")]
    InvalidStateTransition { from: String, to: String },

    #[error("Operation not found: {0}")]
    OperationNotFound(String),

    #[error("Depth limit exceeded: {depth} > {max}")]
    DepthLimitExceeded { depth: u32, max: u32 },

    #[error("Concurrency limit exceeded")]
    ConcurrencyLimitExceeded,

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Sub-agent limit exceeded: max {limit} concurrent sub-agents")]
    SubAgentLimitExceeded { limit: usize },

    #[error("Sub-agent not found: {id}")]
    SubAgentNotFound { id: String },

    #[error("Sub-agent {id} not running (state: {state})")]
    SubAgentNotRunning { id: String, state: String },

    #[error("Invalid tool in access policy: {tool}")]
    InvalidToolAccess { tool: String },

    #[error("Sub-agent spawn failed: {reason}")]
    SubAgentSpawnFailed { reason: String },
}

impl AgentError {
    /// Check if this error should trigger graceful shutdown (vs immediate fail)
    pub fn is_graceful(&self) -> bool {
        matches!(
            self,
            Self::TokenBudgetExceeded { .. }
                | Self::TimeBudgetExceeded { .. }
                | Self::ToolCallBudgetExceeded { .. }
                | Self::MaxTurnsReached { .. }
        )
    }

    /// Check if this error is recoverable (can retry)
    pub fn is_recoverable(&self) -> bool {
        matches!(self, Self::LlmError(_))
    }
}

pub fn store_error(err: impl std::fmt::Display) -> AgentError {
    AgentError::StoreError(store_error_message(err))
}

pub fn invalid_session_id(err: impl std::fmt::Display) -> AgentError {
    AgentError::StoreError(invalid_session_id_message(err))
}

pub fn store_error_message(err: impl std::fmt::Display) -> String {
    err.to_string()
}

pub fn invalid_session_id_message(err: impl std::fmt::Display) -> String {
    format!("Invalid session ID: {}", err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = AgentError::TokenBudgetExceeded {
            used: 10000,
            limit: 5000,
        };
        assert!(err.to_string().contains("10000"));
        assert!(err.to_string().contains("5000"));
    }

    #[test]
    fn test_graceful_errors() {
        assert!(AgentError::TokenBudgetExceeded { used: 0, limit: 0 }.is_graceful());
        assert!(
            AgentError::TimeBudgetExceeded {
                elapsed_secs: 0,
                limit_secs: 0
            }
            .is_graceful()
        );
        assert!(AgentError::MaxTurnsReached { turns: 10 }.is_graceful());

        assert!(!AgentError::Cancelled.is_graceful());
        assert!(!AgentError::LlmError("test".to_string()).is_graceful());
    }

    #[test]
    fn test_recoverable_errors() {
        assert!(AgentError::LlmError("rate limited".to_string()).is_recoverable());
        assert!(!AgentError::Cancelled.is_recoverable());
        assert!(!AgentError::TokenBudgetExceeded { used: 0, limit: 0 }.is_recoverable());
    }
}
