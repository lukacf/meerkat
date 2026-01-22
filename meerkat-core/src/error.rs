//! Agent errors for Meerkat

use crate::types::SessionId;

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
        assert!(AgentError::TimeBudgetExceeded {
            elapsed_secs: 0,
            limit_secs: 0
        }
        .is_graceful());
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
