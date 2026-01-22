//! Agent events for streaming output
//!
//! These events form the streaming API for consumers.

use crate::types::{SessionId, StopReason, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

/// Events emitted during agent execution
///
/// These events form the streaming API for consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    // === Session Lifecycle ===
    /// Agent run started
    RunStarted {
        session_id: SessionId,
        prompt: String,
    },

    /// Agent run completed successfully
    RunCompleted {
        session_id: SessionId,
        result: String,
        usage: Usage,
    },

    /// Agent run failed
    RunFailed {
        session_id: SessionId,
        error: String,
    },

    // === LLM Interaction ===
    /// New turn started (calling LLM)
    TurnStarted {
        turn_number: u32,
    },

    /// Streaming text from the model
    TextDelta {
        delta: String,
    },

    /// Text generation complete for this turn
    TextComplete {
        content: String,
    },

    /// Model requested a tool call
    ToolCallRequested {
        id: String,
        name: String,
        args: Value,
    },

    /// Tool result received (injected into conversation)
    ToolResultReceived {
        id: String,
        name: String,
        is_error: bool,
    },

    /// Turn completed
    TurnCompleted {
        stop_reason: StopReason,
        usage: Usage,
    },

    // === Tool Execution ===
    /// Starting tool execution
    ToolExecutionStarted {
        id: String,
        name: String,
    },

    /// Tool execution completed
    ToolExecutionCompleted {
        id: String,
        name: String,
        result: String,
        is_error: bool,
        duration_ms: u64,
    },

    /// Tool execution timed out
    ToolExecutionTimedOut {
        id: String,
        name: String,
        timeout_ms: u64,
    },

    // === Budget & Checkpointing ===
    /// Budget warning (approaching limits)
    BudgetWarning {
        budget_type: BudgetType,
        used: u64,
        limit: u64,
        percent: f32,
    },

    /// Session checkpoint saved
    CheckpointSaved {
        session_id: SessionId,
        path: Option<PathBuf>,
    },

    // === Retry Events ===
    /// Retrying after error
    Retrying {
        attempt: u32,
        max_attempts: u32,
        error: String,
        delay_ms: u64,
    },
}

/// Type of budget being tracked
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetType {
    Tokens,
    Time,
    ToolCalls,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_event_json_schema() {
        // Test all event variants serialize correctly
        let events = vec![
            AgentEvent::RunStarted {
                session_id: SessionId::new(),
                prompt: "Hello".to_string(),
            },
            AgentEvent::TextDelta {
                delta: "chunk".to_string(),
            },
            AgentEvent::TurnStarted { turn_number: 1 },
            AgentEvent::TurnCompleted {
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
            AgentEvent::ToolCallRequested {
                id: "tc_1".to_string(),
                name: "read_file".to_string(),
                args: serde_json::json!({"path": "/tmp/test"}),
            },
            AgentEvent::ToolResultReceived {
                id: "tc_1".to_string(),
                name: "read_file".to_string(),
                is_error: false,
            },
            AgentEvent::BudgetWarning {
                budget_type: BudgetType::Tokens,
                used: 8000,
                limit: 10000,
                percent: 0.8,
            },
            AgentEvent::CheckpointSaved {
                session_id: SessionId::new(),
                path: Some(PathBuf::from("/tmp/session.jsonl")),
            },
            AgentEvent::Retrying {
                attempt: 1,
                max_attempts: 3,
                error: "Rate limited".to_string(),
                delay_ms: 1000,
            },
            AgentEvent::RunCompleted {
                session_id: SessionId::new(),
                result: "Done".to_string(),
                usage: Usage {
                    input_tokens: 100,
                    output_tokens: 50,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            },
            AgentEvent::RunFailed {
                session_id: SessionId::new(),
                error: "Budget exceeded".to_string(),
            },
        ];

        for event in events {
            let json = serde_json::to_value(&event).unwrap();

            // All events should have a "type" field
            assert!(json.get("type").is_some(), "Event missing type field: {:?}", event);

            // Should roundtrip
            let roundtrip: AgentEvent = serde_json::from_value(json.clone()).unwrap();
            let json2 = serde_json::to_value(&roundtrip).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn test_budget_type_serialization() {
        assert_eq!(
            serde_json::to_value(BudgetType::Tokens).unwrap(),
            "tokens"
        );
        assert_eq!(
            serde_json::to_value(BudgetType::Time).unwrap(),
            "time"
        );
        assert_eq!(
            serde_json::to_value(BudgetType::ToolCalls).unwrap(),
            "tool_calls"
        );
    }
}
