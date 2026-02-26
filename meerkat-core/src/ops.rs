//! Async operation types for Meerkat
//!
//! Unified abstraction for tool calls, shell commands, and sub-agents.

use crate::budget::BudgetLimits;
use crate::types::Message;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for an operation
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OperationId(pub Uuid);

impl OperationId {
    /// Create a new operation ID
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for OperationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for OperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// What kind of work the operation performs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkKind {
    /// MCP or internal tool call
    ToolCall,
    /// Shell command execution
    ShellCommand,
    /// Sub-agent (spawn or fork)
    SubAgent,
}

/// Shape of the operation's result
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResultShape {
    /// Single result value
    Single,
    /// Streaming output (progress events)
    Stream,
    /// Multiple results (e.g., fork branches)
    Batch,
}

/// How much context a sub-agent receives
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ContextStrategy {
    /// Complete conversation history (Fork default)
    #[default]
    FullHistory,
    /// Last N turns from parent
    LastTurns(u32),
    /// Compressed summary of conversation
    Summary { max_tokens: u32 },
    /// Explicit message list
    Custom { messages: Vec<Message> },
}

/// How to allocate budget when forking
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ForkBudgetPolicy {
    /// Split remaining budget equally among branches
    #[default]
    Equal,
    /// Split proportionally based on weights
    Proportional,
    /// Fixed budget per branch
    Fixed(u64),
    /// Give all remaining budget to each branch
    Remaining,
}

/// Tool access control for sub-agents
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ToolAccessPolicy {
    /// Inherit all tools from parent
    #[default]
    Inherit,
    /// Only allow specific tools
    AllowList(Vec<String>),
    /// Block specific tools
    DenyList(Vec<String>),
}

/// Policy for operation execution
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperationPolicy {
    /// Timeout for this operation
    pub timeout_ms: Option<u64>,
    /// Whether to cancel on parent cancellation
    pub cancel_on_parent_cancel: bool,
    /// Whether to include in checkpoints
    pub checkpoint_results: bool,
}

/// Complete operation specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationSpec {
    pub id: OperationId,
    pub kind: WorkKind,
    pub result_shape: ResultShape,
    pub policy: OperationPolicy,
    pub budget_reservation: BudgetLimits,
    pub depth: u32,
    pub depends_on: Vec<OperationId>,
    pub context: Option<ContextStrategy>,
    pub tool_access: Option<ToolAccessPolicy>,
}

/// Result of a completed operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationResult {
    pub id: OperationId,
    pub content: String,
    pub is_error: bool,
    pub duration_ms: u64,
    pub tokens_used: u64,
}

/// Events from operations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OpEvent {
    /// Operation started executing
    Started { id: OperationId, kind: WorkKind },

    /// Progress update (for streaming operations)
    Progress {
        id: OperationId,
        message: String,
        percent: Option<f32>,
    },

    /// Operation completed successfully
    Completed {
        id: OperationId,
        result: OperationResult,
    },

    /// Operation failed
    Failed { id: OperationId, error: String },

    /// Operation was cancelled
    Cancelled { id: OperationId },
}

/// Concurrency limits for operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcurrencyLimits {
    /// Maximum sub-agent nesting depth
    pub max_depth: u32,
    /// Maximum concurrent operations (all types)
    pub max_concurrent_ops: usize,
    /// Maximum concurrent sub-agents specifically
    pub max_concurrent_agents: usize,
    /// Maximum children per parent agent
    pub max_children_per_agent: usize,
}

impl Default for ConcurrencyLimits {
    fn default() -> Self {
        Self {
            max_depth: 3,
            max_concurrent_ops: 32,
            max_concurrent_agents: 8,
            max_children_per_agent: 5,
        }
    }
}

/// Specification for spawning a new sub-agent
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SpawnSpec {
    /// The prompt/task for the sub-agent
    pub prompt: String,
    /// How much context the sub-agent receives
    pub context: ContextStrategy,
    /// Which tools the sub-agent can access
    pub tool_access: ToolAccessPolicy,
    /// Budget allocation for the sub-agent
    pub budget: BudgetLimits,
    /// If false, sub-agent cannot spawn/fork further
    pub allow_spawn: bool,
    /// System prompt override (None = inherit from parent)
    pub system_prompt: Option<String>,
}

/// A branch in a fork operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkBranch {
    /// Identifier for this branch
    pub name: String,
    /// The prompt/task for this branch
    pub prompt: String,
    /// Tool access override (None = inherit)
    pub tool_access: Option<ToolAccessPolicy>,
}

/// State of a running sub-agent
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubAgentState {
    /// Sub-agent is running
    Running,
    /// Sub-agent completed successfully
    Completed,
    /// Sub-agent failed with error
    Failed,
    /// Sub-agent was cancelled
    Cancelled,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_id_encoding() {
        let id = OperationId::new();
        let json = serde_json::to_string(&id).unwrap();

        let parsed: OperationId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn test_work_kind_serialization() {
        assert_eq!(
            serde_json::to_value(WorkKind::ToolCall).unwrap(),
            "tool_call"
        );
        assert_eq!(
            serde_json::to_value(WorkKind::ShellCommand).unwrap(),
            "shell_command"
        );
        assert_eq!(
            serde_json::to_value(WorkKind::SubAgent).unwrap(),
            "sub_agent"
        );
    }

    #[test]
    fn test_context_strategy_serialization() {
        let full = ContextStrategy::FullHistory;
        let json = serde_json::to_value(&full).unwrap();
        assert_eq!(json["type"], "full_history");

        let last = ContextStrategy::LastTurns(5);
        let json = serde_json::to_value(&last).unwrap();
        assert_eq!(json["type"], "last_turns");
        // Adjacently-tagged: {"type": "last_turns", "value": 5}
        assert_eq!(json["value"], 5);

        let summary = ContextStrategy::Summary { max_tokens: 1000 };
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["type"], "summary");
        // Adjacently-tagged struct variant: {"type": "summary", "value": {"max_tokens": 1000}}
        assert_eq!(json["value"]["max_tokens"], 1000);

        // Roundtrip
        let parsed: ContextStrategy = serde_json::from_value(json).unwrap();
        match parsed {
            ContextStrategy::Summary { max_tokens } => assert_eq!(max_tokens, 1000),
            _ => unreachable!("Wrong variant"),
        }
    }

    #[test]
    fn test_fork_budget_policy_serialization() {
        let policies = vec![
            (ForkBudgetPolicy::Equal, "equal"),
            (ForkBudgetPolicy::Proportional, "proportional"),
            (ForkBudgetPolicy::Remaining, "remaining"),
        ];

        for (policy, expected_type) in policies {
            let json = serde_json::to_value(&policy).unwrap();
            assert_eq!(json["type"], expected_type);
        }

        let fixed = ForkBudgetPolicy::Fixed(5000);
        let json = serde_json::to_value(&fixed).unwrap();
        assert_eq!(json["type"], "fixed");
        // Adjacently-tagged: {"type": "fixed", "value": 5000}
        assert_eq!(json["value"], 5000);

        // Roundtrip
        let parsed: ForkBudgetPolicy = serde_json::from_value(json).unwrap();
        match parsed {
            ForkBudgetPolicy::Fixed(tokens) => assert_eq!(tokens, 5000),
            _ => unreachable!("Wrong variant"),
        }
    }

    #[test]
    fn test_tool_access_policy_serialization() {
        let inherit = ToolAccessPolicy::Inherit;
        let json = serde_json::to_value(&inherit).unwrap();
        assert_eq!(json["type"], "inherit");

        let allow =
            ToolAccessPolicy::AllowList(vec!["read_file".to_string(), "write_file".to_string()]);
        let json = serde_json::to_value(&allow).unwrap();
        assert_eq!(json["type"], "allow_list");
        // Adjacently-tagged: {"type": "allow_list", "value": [...]}
        assert!(json["value"].is_array());

        let deny = ToolAccessPolicy::DenyList(vec!["dangerous_tool".to_string()]);
        let json = serde_json::to_value(&deny).unwrap();
        assert_eq!(json["type"], "deny_list");
        assert!(json["value"].is_array());

        // Roundtrip
        let parsed: ToolAccessPolicy = serde_json::from_value(json).unwrap();
        match parsed {
            ToolAccessPolicy::DenyList(tools) => {
                assert_eq!(tools.len(), 1);
                assert_eq!(tools[0], "dangerous_tool");
            }
            _ => unreachable!("Wrong variant"),
        }
    }

    #[test]
    fn test_op_event_serialization() {
        let events = vec![
            OpEvent::Started {
                id: OperationId::new(),
                kind: WorkKind::ToolCall,
            },
            OpEvent::Progress {
                id: OperationId::new(),
                message: "50% complete".to_string(),
                percent: Some(0.5),
            },
            OpEvent::Completed {
                id: OperationId::new(),
                result: OperationResult {
                    id: OperationId::new(),
                    content: "result".to_string(),
                    is_error: false,
                    duration_ms: 100,
                    tokens_used: 50,
                },
            },
            OpEvent::Failed {
                id: OperationId::new(),
                error: "timeout".to_string(),
            },
            OpEvent::Cancelled {
                id: OperationId::new(),
            },
        ];

        for event in events {
            let json = serde_json::to_value(&event).unwrap();
            assert!(json.get("type").is_some());

            // Roundtrip
            let _: OpEvent = serde_json::from_value(json).unwrap();
        }
    }

    #[test]
    fn test_concurrency_limits_default() {
        let limits = ConcurrencyLimits::default();
        assert_eq!(limits.max_depth, 3);
        assert_eq!(limits.max_concurrent_ops, 32);
        assert_eq!(limits.max_concurrent_agents, 8);
        assert_eq!(limits.max_children_per_agent, 5);
    }
}
