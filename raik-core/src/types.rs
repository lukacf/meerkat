//! Core types for RAIK
//!
//! These types form the representation boundary for session persistence and wire formats.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Unique identifier for a session (UUID v7 for time-ordering)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// Create a new session ID using UUID v7
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Create from an existing UUID
    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    /// Parse from string
    pub fn parse(s: &str) -> Result<Self, uuid::Error> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A message in the conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Message {
    /// System prompt (injected at start)
    System(SystemMessage),
    /// User input
    User(UserMessage),
    /// Assistant response (may include tool calls)
    Assistant(AssistantMessage),
    /// Results from tool execution
    #[serde(rename = "tool_results")]
    ToolResults { results: Vec<ToolResult> },
}

/// System message content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SystemMessage {
    pub content: String,
}

/// User message content
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct UserMessage {
    pub content: String,
}

/// Assistant message with potential tool calls
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AssistantMessage {
    /// Text content (may be empty if only tool calls)
    pub content: String,
    /// Tool calls requested by the model
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// How the turn ended
    pub stop_reason: StopReason,
    /// Token usage for this turn
    pub usage: Usage,
}

/// A tool call requested by the model
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolCall {
    /// Unique ID for this tool use (from the model)
    pub id: String,
    /// Name of the tool to invoke
    pub name: String,
    /// Arguments as JSON
    pub args: Value,
}

/// Result of executing a tool
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolResult {
    /// Matches the tool_call.id
    pub tool_use_id: String,
    /// Content returned by the tool
    pub content: String,
    /// Whether this is an error result
    #[serde(default)]
    pub is_error: bool,
}

/// Why the model stopped generating
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// Model finished naturally
    #[default]
    EndTurn,
    /// Model wants to call tools
    ToolUse,
    /// Hit max output tokens
    MaxTokens,
    /// Hit a stop sequence
    StopSequence,
    /// Blocked by content filter
    ContentFilter,
    /// Request was cancelled
    Cancelled,
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
}

impl Usage {
    /// Get total tokens used
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    /// Accumulate usage from another
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        if let Some(c) = other.cache_creation_tokens {
            *self.cache_creation_tokens.get_or_insert(0) += c;
        }
        if let Some(c) = other.cache_read_tokens {
            *self.cache_read_tokens.get_or_insert(0) += c;
        }
    }
}

/// Tool definition for the LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Result of a successful agent run
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RunResult {
    /// Final text response
    pub text: String,
    /// Session ID for resumption
    pub session_id: SessionId,
    /// Total token usage
    pub usage: Usage,
    /// Number of turns taken
    pub turns: u32,
    /// Number of tool calls made
    pub tool_calls: u32,
}

/// Reference to an artifact stored externally
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ArtifactRef {
    /// Unique artifact identifier
    pub id: String,
    /// Session this artifact belongs to
    pub session_id: SessionId,
    /// Size in bytes
    pub size_bytes: u64,
    /// Time-to-live in seconds (None = permanent)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
    /// Format version for migrations
    pub version: u32,
}

#[cfg(test)]
mod tests;
