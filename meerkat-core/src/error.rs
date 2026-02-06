//! Core error types for Meerkat

use crate::hooks::{HookPoint, HookReasonCode};
use crate::types::SessionId;

#[derive(Debug, Clone, PartialEq)]
pub enum LlmFailureReason {
    RateLimited {
        retry_after: Option<std::time::Duration>,
    },
    ContextExceeded {
        max: u32,
        requested: u32,
    },
    AuthError,
    InvalidModel(String),
    ProviderError(serde_json::Value),
}

/// Errors that can occur during tool validation
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum ToolValidationError {
    /// The requested tool was not found
    #[error("Tool not found: {name}")]
    NotFound { name: String },
    /// The tool arguments failed validation
    #[error("Invalid arguments for tool '{name}': {reason}")]
    InvalidArguments { name: String, reason: String },
}

impl ToolValidationError {
    pub fn not_found(name: impl Into<String>) -> Self {
        Self::NotFound { name: name.into() }
    }
    pub fn invalid_arguments(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidArguments {
            name: name.into(),
            reason: reason.into(),
        }
    }
}

/// Error returned by tool dispatch operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ToolError {
    /// The requested tool was not found
    #[error("Tool not found: {name}")]
    NotFound { name: String },

    /// The tool exists but is currently unavailable
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

    /// Tool call must be routed externally (callback pending)
    ///
    /// This variant signals that a tool call cannot be handled internally
    /// and must be routed to an external handler. The payload contains
    /// serialized information about the pending tool call.
    #[error("Callback pending for tool '{tool_name}'")]
    CallbackPending {
        tool_name: String,
        args: serde_json::Value,
    },
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
            Self::CallbackPending { .. } => "callback_pending",
        }
    }

    pub fn to_error_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "error": self.error_code(),
            "message": self.to_string(),
        })
    }

    pub fn not_found(name: impl Into<String>) -> Self {
        Self::NotFound { name: name.into() }
    }
    pub fn unavailable(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Unavailable {
            name: name.into(),
            reason: reason.into(),
        }
    }
    pub fn invalid_arguments(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidArguments {
            name: name.into(),
            reason: reason.into(),
        }
    }
    pub fn execution_failed(message: impl Into<String>) -> Self {
        Self::ExecutionFailed {
            message: message.into(),
        }
    }
    pub fn timeout(name: impl Into<String>, timeout_ms: u64) -> Self {
        Self::Timeout {
            name: name.into(),
            timeout_ms,
        }
    }
    pub fn access_denied(name: impl Into<String>) -> Self {
        Self::AccessDenied { name: name.into() }
    }
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    /// Create a callback pending error for external tool routing
    pub fn callback_pending(tool_name: impl Into<String>, args: serde_json::Value) -> Self {
        Self::CallbackPending {
            tool_name: tool_name.into(),
            args,
        }
    }

    /// Check if this is a callback pending error
    pub fn is_callback_pending(&self) -> bool {
        matches!(self, Self::CallbackPending { .. })
    }

    /// Extract callback pending info if this is a CallbackPending error
    pub fn as_callback_pending(&self) -> Option<(&str, &serde_json::Value)> {
        match self {
            Self::CallbackPending { tool_name, args } => Some((tool_name, args)),
            _ => None,
        }
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
    #[error("LLM error ({provider}): {message}")]
    Llm {
        provider: &'static str,
        reason: LlmFailureReason,
        message: String,
    },
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
    #[error("Internal error: {0}")]
    InternalError(String),

    /// A tool call must be routed externally (callback pending)
    #[error("Callback pending for tool '{tool_name}'")]
    CallbackPending {
        tool_name: String,
        args: serde_json::Value,
    },

    /// Structured output validation failed after retries
    #[error("Structured output validation failed after {attempts} attempts: {reason}")]
    StructuredOutputValidationFailed {
        attempts: u32,
        reason: String,
        last_output: String,
    },

    /// Invalid output schema provided
    #[error("Invalid output schema: {0}")]
    InvalidOutputSchema(String),

    #[error("Hook denied at {point:?}: {reason_code:?} - {message}")]
    HookDenied {
        point: HookPoint,
        reason_code: HookReasonCode,
        message: String,
        payload: Option<serde_json::Value>,
    },

    #[error("Hook '{hook_id}' timed out after {timeout_ms}ms")]
    HookTimeout { hook_id: String, timeout_ms: u64 },

    #[error("Hook execution failed for '{hook_id}': {reason}")]
    HookExecutionFailed { hook_id: String, reason: String },

    #[error("Hook configuration invalid: {reason}")]
    HookConfigInvalid { reason: String },
}

impl AgentError {
    pub fn llm(
        provider: &'static str,
        reason: LlmFailureReason,
        message: impl Into<String>,
    ) -> Self {
        Self::Llm {
            provider,
            reason,
            message: message.into(),
        }
    }
    pub fn is_graceful(&self) -> bool {
        matches!(
            self,
            Self::TokenBudgetExceeded { .. }
                | Self::TimeBudgetExceeded { .. }
                | Self::ToolCallBudgetExceeded { .. }
                | Self::MaxTurnsReached { .. }
        )
    }
    pub fn is_recoverable(&self) -> bool {
        match self {
            Self::Llm { reason, .. } => match reason {
                LlmFailureReason::RateLimited { .. } => true,
                LlmFailureReason::ProviderError(value) => {
                    value.get("retryable").and_then(|v| v.as_bool()) == Some(true)
                }
                _ => false,
            },
            _ => false,
        }
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
