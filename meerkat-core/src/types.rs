//! Core types for Meerkat
//!
//! These types form the representation boundary for session persistence and wire formats.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Map, Value};
use std::fmt;
use uuid::Uuid;

use crate::schema::{
    CompiledSchema, MeerkatSchema, SchemaCompat, SchemaError, SchemaFormat, SchemaWarning,
};
use crate::Provider;

// ===========================================================================
// New ordered transcript types (spec section 3.1)
// ===========================================================================

/// Provider-specific metadata for replay continuity.
/// Typed enum prevents runtime "is this an object?" errors.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "provider", rename_all = "snake_case")]
pub enum ProviderMeta {
    /// Anthropic thinking block signature
    Anthropic {
        /// Opaque signature for thinking continuity
        signature: String,
    },
    /// Gemini thought signature for tool calls
    Gemini {
        /// Opaque signature for tool call continuity
        #[serde(rename = "thoughtSignature")]
        thought_signature: String,
    },
    /// OpenAI reasoning item metadata for stateless replay.
    /// Both `id` and `encrypted_content` are required for faithful round-trip.
    OpenAi {
        /// Reasoning item ID (required by OpenAI schema)
        id: String,
        /// Encrypted reasoning tokens for continuity
        #[serde(skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
    },
}

/// A block of content in an assistant message, preserving order.
///
/// Uses adjacently tagged serde representation to support `Box<RawValue>` in ToolUse.
/// Internally tagged enums don't work with RawValue due to buffering requirements.
#[non_exhaustive]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "block_type", content = "data", rename_all = "snake_case")]
pub enum AssistantBlock {
    /// Plain text output.
    /// Note: Gemini may attach thoughtSignature to trailing text parts for continuity.
    Text {
        text: String,
        /// Provider metadata (Gemini thoughtSignature on text parts)
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<Box<ProviderMeta>>,
    },

    /// Visible reasoning/thinking notes emitted by the model
    Reasoning {
        /// Human-readable text (may be empty if only encrypted content)
        #[serde(default)]
        text: String,
        /// Provider continuity metadata (typed, not arbitrary JSON)
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<Box<ProviderMeta>>,
    },

    /// Tool use request
    ToolUse {
        id: String,
        name: String,
        /// Arguments as raw JSON - only dispatcher parses this
        args: Box<RawValue>,
        /// Provider continuity metadata
        #[serde(skip_serializing_if = "Option::is_none")]
        meta: Option<Box<ProviderMeta>>,
    },
}

impl PartialEq for AssistantBlock {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                AssistantBlock::Text { text: t1, meta: m1 },
                AssistantBlock::Text { text: t2, meta: m2 },
            ) => t1 == t2 && m1 == m2,
            (
                AssistantBlock::Reasoning { text: t1, meta: m1 },
                AssistantBlock::Reasoning { text: t2, meta: m2 },
            ) => t1 == t2 && m1 == m2,
            (
                AssistantBlock::ToolUse { id: i1, name: n1, args: a1, meta: m1 },
                AssistantBlock::ToolUse { id: i2, name: n2, args: a2, meta: m2 },
            ) => i1 == i2 && n1 == n2 && a1.get() == a2.get() && m1 == m2,
            _ => false,
        }
    }
}

/// Borrowed view into a tool call block. Zero allocations.
#[derive(Debug, Clone, Copy)]
pub struct ToolCallView<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub args: &'a RawValue,
}

impl ToolCallView<'_> {
    /// Parse args into a concrete type. Called by dispatcher, not core.
    ///
    /// # Errors
    /// Returns `serde_json::Error` if args don't match expected schema.
    pub fn parse_args<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(self.args.get())
    }
}

/// Iterator over tool calls in an AssistantMessage. Zero allocations.
pub struct ToolCallIter<'a> {
    inner: std::slice::Iter<'a, AssistantBlock>,
}

impl<'a> Iterator for ToolCallIter<'a> {
    type Item = ToolCallView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next()? {
                AssistantBlock::ToolUse { id, name, args, .. } => {
                    return Some(ToolCallView { id, name, args });
                }
                _ => continue,
            }
        }
    }
}

// ===========================================================================
// Legacy types (will be migrated in later phases)
// ===========================================================================

/// Configuration for structured output extraction.
///
/// When provided to an agent, the agent will perform an extraction turn after
/// completing the agentic work, forcing the LLM to output validated JSON that
/// conforms to the provided schema. The extraction JSON becomes the final
/// response text (schema-only) in [`RunResult`].
#[derive(Debug, Clone, Serialize)]
pub struct OutputSchema {
    /// The JSON schema that the output must conform to
    pub schema: MeerkatSchema,
    /// Optional name for the schema (used by some providers)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether to enforce strict schema validation (provider-specific)
    #[serde(default)]
    pub strict: bool,
    /// Compatibility mode for provider lowering (lossy or strict)
    #[serde(default)]
    pub compat: SchemaCompat,
    /// Schema format version
    #[serde(default)]
    pub format: SchemaFormat,
}

impl OutputSchema {
    /// Create a new output schema
    pub fn new(schema: Value) -> Result<Self, SchemaError> {
        Ok(Self {
            schema: MeerkatSchema::new(schema)?,
            name: None,
            strict: false,
            compat: SchemaCompat::default(),
            format: SchemaFormat::default(),
        })
    }

    /// Set the schema name
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Enable strict mode for schema validation
    pub fn strict(mut self) -> Self {
        self.strict = true;
        self
    }

    /// Set compatibility mode for provider lowering
    pub fn with_compat(mut self, compat: SchemaCompat) -> Self {
        self.compat = compat;
        self
    }

    /// Set the schema format
    pub fn with_format(mut self, format: SchemaFormat) -> Self {
        self.format = format;
        self
    }

    /// Parse from a JSON string (wrapper or raw schema).
    pub fn from_json_str(raw: &str) -> Result<Self, SchemaError> {
        let value: Value = serde_json::from_str(raw).map_err(|_| SchemaError::InvalidRoot)?;
        Self::from_json_value(value)
    }

    /// Parse from a JSON value (wrapper or raw schema).
    pub fn from_json_value(value: Value) -> Result<Self, SchemaError> {
        match value {
            Value::Object(obj) if is_wrapped_schema(&obj) => {
                let wrapped: OutputSchemaWire =
                    serde_json::from_value(Value::Object(obj)).map_err(|_| SchemaError::InvalidRoot)?;
                Ok(Self {
                    schema: MeerkatSchema::new(wrapped.schema)?,
                    name: wrapped.name,
                    strict: wrapped.strict,
                    compat: wrapped.compat,
                    format: wrapped.format,
                })
            }
            other => OutputSchema::new(other),
        }
    }

    /// Create from a Rust type via schemars.
    pub fn from_type<T: schemars::JsonSchema>() -> Result<Self, SchemaError> {
        let schema = schemars::schema_for!(T);
        let mut value = serde_json::to_value(&schema).map_err(|_| SchemaError::InvalidRoot)?;
        // Ensure object schemas always have `properties` and `required` keys.
        if let Value::Object(ref mut obj) = value {
            if obj.get("type").and_then(Value::as_str) == Some("object") {
                obj.entry("properties".to_string())
                    .or_insert_with(|| Value::Object(Map::new()));
                obj.entry("required".to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
            }
        }
        OutputSchema::new(value)
    }

    /// Compile this schema for a provider using the configured compatibility mode.
    pub fn compile_for(&self, provider: Provider) -> Result<CompiledSchema, SchemaError> {
        self.schema.compile_for(provider, self.compat)
    }

    /// Convert to a JSON value for provider params.
    pub fn to_value(&self) -> Value {
        let mut obj = Map::new();
        obj.insert("schema".to_string(), self.schema.as_value().clone());
        if let Some(name) = &self.name {
            obj.insert("name".to_string(), Value::String(name.clone()));
        }
        obj.insert("strict".to_string(), Value::Bool(self.strict));
        obj.insert(
            "compat".to_string(),
            serde_json::to_value(self.compat).unwrap_or(Value::String("lossy".to_string())),
        );
        obj.insert(
            "format".to_string(),
            serde_json::to_value(self.format)
                .unwrap_or(Value::String("meerkat_v1".to_string())),
        );
        Value::Object(obj)
    }
}

impl<'de> Deserialize<'de> for OutputSchema {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        OutputSchema::from_json_value(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Deserialize)]
struct OutputSchemaWire {
    pub schema: Value,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub compat: SchemaCompat,
    #[serde(default)]
    pub format: SchemaFormat,
}

fn is_wrapped_schema(obj: &Map<String, Value>) -> bool {
    let has_schema = obj.contains_key("schema");
    let has_marker =
        obj.contains_key("compat") || obj.contains_key("strict") || obj.contains_key("name");
    let has_format_marker = obj
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|f| f == "meerkat_v1");
    let only_wrapper_keys = obj.keys().all(|key| {
        matches!(key.as_str(), "schema" | "name" | "strict" | "compat" | "format")
    });
    has_schema && (has_marker || has_format_marker || only_wrapper_keys)
}

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
    /// Assistant response (may include tool calls) - legacy format
    Assistant(AssistantMessage),
    /// Assistant response with ordered blocks - new format (spec v2)
    #[serde(rename = "block_assistant")]
    BlockAssistant(BlockAssistantMessage),
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

/// New assistant message with ordered blocks - no billing metadata.
///
/// This is the new format for storing assistant messages with full block ordering.
/// The existing `AssistantMessage` is preserved for backwards compatibility during
/// the migration period.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlockAssistantMessage {
    /// Ordered sequence of content blocks
    pub blocks: Vec<AssistantBlock>,
    /// How the turn ended
    pub stop_reason: StopReason,
}

// Usage is NOT part of BlockAssistantMessage.
// It's billing metadata returned separately from LLM calls.
// See LlmResponse in meerkat-client.

impl BlockAssistantMessage {
    /// Iterate over tool calls without allocation.
    pub fn tool_calls(&self) -> ToolCallIter<'_> {
        ToolCallIter { inner: self.blocks.iter() }
    }

    /// Check if any tool calls are present.
    pub fn has_tool_calls(&self) -> bool {
        self.blocks.iter().any(|b| matches!(b, AssistantBlock::ToolUse { .. }))
    }

    /// Get tool use block by ID.
    pub fn get_tool_use(&self, id: &str) -> Option<ToolCallView<'_>> {
        self.tool_calls().find(|tc| tc.id == id)
    }

    /// Iterate over text blocks without allocation.
    pub fn text_blocks(&self) -> impl Iterator<Item = &str> {
        self.blocks.iter().filter_map(|b| match b {
            AssistantBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
    }
}

/// Display implementation for text content - zero intermediate allocations.
impl fmt::Display for BlockAssistantMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for block in &self.blocks {
            if let AssistantBlock::Text { text, .. } = block {
                f.write_str(text)?;
            }
        }
        Ok(())
    }
}

/// Assistant message with potential tool calls (legacy format)
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
    /// Thought signature for Gemini 3+ models (must be passed back with tool result)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

impl ToolCall {
    /// Create a new tool call (without thought signature)
    pub fn new(id: String, name: String, args: Value) -> Self {
        Self {
            id,
            name,
            args,
            thought_signature: None,
        }
    }

    /// Create a new tool call with thought signature (for Gemini 3+)
    pub fn with_thought_signature(
        id: String,
        name: String,
        args: Value,
        thought_signature: String,
    ) -> Self {
        Self {
            id,
            name,
            args,
            thought_signature: Some(thought_signature),
        }
    }
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
    /// Thought signature for Gemini 3+ (must be passed back from original ToolCall)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

impl ToolResult {
    /// Create a new tool result (without thought signature)
    pub fn new(tool_use_id: String, content: String, is_error: bool) -> Self {
        Self {
            tool_use_id,
            content,
            is_error,
            thought_signature: None,
        }
    }

    /// Create a new tool result with thought signature (for Gemini 3+)
    pub fn with_thought_signature(
        tool_use_id: String,
        content: String,
        is_error: bool,
        thought_signature: String,
    ) -> Self {
        Self {
            tool_use_id,
            content,
            is_error,
            thought_signature: Some(thought_signature),
        }
    }

    /// Create a tool result from a ToolCall (preserves thought_signature)
    pub fn from_tool_call(tool_call: &ToolCall, content: String, is_error: bool) -> Self {
        Self {
            tool_use_id: tool_call.id.clone(),
            content,
            is_error,
            thought_signature: tool_call.thought_signature.clone(),
        }
    }
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

/// Security mode for tool execution (e.g. shell)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SecurityMode {
    /// No restrictions. (Default)
    #[default]
    Unrestricted,
    /// Only allow commands matching specified patterns.
    AllowList,
    /// Block commands matching specified patterns.
    DenyList,
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
    /// Final text response.
    ///
    /// When `output_schema` is set, this is the raw JSON string produced by the
    /// extraction turn (schema-only; no additional text).
    pub text: String,
    /// Session ID for resumption
    pub session_id: SessionId,
    /// Total token usage
    pub usage: Usage,
    /// Number of turns taken
    pub turns: u32,
    /// Number of tool calls made
    pub tool_calls: u32,
    /// Structured output (if output_schema was provided and extraction succeeded).
    ///
    /// This is the parsed JSON value corresponding to `text` when schema
    /// extraction is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
    /// Warnings produced during schema compilation (if any).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_warnings: Option<Vec<SchemaWarning>>,
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
