# API Reference

This document provides a comprehensive reference for all public types, traits, and functions in Meerkat.

## Module: `meerkat`

The main entry point. Re-exports types from all sub-crates.

### SDK Helpers

AgentFactory is the shared entry point for SDK construction:

```rust
// Shared wiring for SDK, CLI, MCP, REST, and tests.
pub struct AgentFactory { /* fields omitted */ }

impl AgentFactory {
    /// Create a new factory with the required session store path.
    pub fn new(store_path: impl Into<PathBuf>) -> Self

    /// Build an LLM client for a provider with optional base URL override.
    pub fn build_llm_client(
        &self,
        provider: Provider,
        api_key: Option<String>,
        base_url: Option<String>,
    ) -> Result<Arc<dyn LlmClient>, FactoryError>

    /// Build an LLM adapter for the provided client/model.
    pub fn build_llm_adapter(&self, client: Arc<dyn LlmClient>, model: impl Into<String>)
        -> LlmClientAdapter

    /// Build a shared builtin dispatcher using the provided config.
    pub fn build_builtin_dispatcher(
        &self,
        store: Arc<dyn TaskStore>,
        config: BuiltinToolConfig,
        shell_config: Option<ShellConfig>,
        external: Option<Arc<dyn AgentToolDispatcher>>,
        session_id: Option<String>,
    ) -> Result<Arc<dyn AgentToolDispatcher>, CompositeDispatcherError>
}
```

### Prelude

Import common types:

```rust
pub use meerkat::prelude::*;

// Includes:
// - Agent, AgentBuilder
// - AgentLlmClient, AgentToolDispatcher, AgentSessionStore
// - Session, Message, ToolDef, ToolCall, ToolResult
// - BudgetLimits, RetryPolicy
// - RunResult, AgentEvent, AgentError
// - SessionId, Usage
```

---

## Module: `meerkat_core`

Core agent implementation and types.

### Agent

The main agent execution engine:

```rust
pub struct Agent {
    // Private fields
}

impl Agent {
    /// Run the agent with a user message
    pub async fn run(&mut self, message: String) -> Result<RunResult, AgentError>

    /// Run with an event channel for streaming
    pub async fn run_with_events(
        &mut self,
        message: String,
        tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, AgentError>

    /// Cancel the running agent
    pub fn cancel(&self)

    /// Get the current session
    pub fn session(&self) -> &Session

    /// Get the session ID
    pub fn session_id(&self) -> &SessionId
}
```

### AgentBuilder

Builder pattern for agent construction:

```rust
pub struct AgentBuilder {
    // Private fields
}

impl AgentBuilder {
    /// Create a new builder
    pub fn new() -> Self

    /// Set the model name
    pub fn model(self, model: impl Into<String>) -> Self

    /// Set the system prompt
    pub fn system_prompt(self, prompt: impl Into<String>) -> Self

    /// Set max tokens per turn
    pub fn max_tokens_per_turn(self, max: u32) -> Self

    /// Set budget limits
    pub fn budget(self, limits: BudgetLimits) -> Self

    /// Set retry policy
    pub fn retry_policy(self, policy: RetryPolicy) -> Self

    /// Resume an existing session
    pub fn resume_session(self, session: Session) -> Self

    /// Attach hook engine implementation
    pub fn with_hook_engine(self, hook_engine: Arc<dyn HookEngine>) -> Self

    /// Apply run-scoped hook overrides
    pub fn with_hook_run_overrides(self, overrides: HookRunOverrides) -> Self

    /// Build the agent with dependencies
    pub fn build(
        self,
        llm: Arc<dyn AgentLlmClient>,
        tools: Arc<dyn AgentToolDispatcher>,
        store: Arc<dyn AgentSessionStore>,
    ) -> Agent
}
```

### Hook Contracts

Core hook interfaces are exposed from `meerkat_core` and re-exported by `meerkat`. Hooks execute at defined points in the agent loop. Each hook entry specifies a point, execution mode (foreground/background), capability (observe/guardrail/rewrite), and a runtime adapter (in-process, command, or HTTP). See `meerkat-hooks` for the `DefaultHookEngine` implementation.

```rust
pub enum HookPoint {
    RunStarted,
    RunCompleted,
    RunFailed,
    PreLlmRequest,
    PostLlmResponse,
    PreToolExecution,
    PostToolExecution,
    TurnBoundary,
}

pub enum HookExecutionMode {
    Foreground,
    Background,
}

pub enum HookCapability {
    Observe,
    Guardrail,
    Rewrite,
}

pub struct HookRunOverrides {
    /// Additional hook entries to register for this run only.
    pub entries: Vec<HookEntryConfig>,
    /// Hook IDs to disable for this run.
    pub disable: Vec<HookId>,
}

/// Newtype wrapper for hook identifiers.
pub struct HookId(pub String);

/// Configuration for a single hook entry.
pub struct HookEntryConfig {
    pub id: HookId,
    pub point: HookPoint,
    pub priority: i32,
    pub mode: HookExecutionMode,
    pub capability: HookCapability,
    pub enabled: bool,
    pub runtime: HookRuntimeConfig,
    pub failure_policy: Option<HookFailurePolicy>,
    pub timeout_ms: Option<u64>,
}

pub enum HookFailurePolicy {
    /// Continue execution on hook failure. Default for `Observe` capability.
    FailOpen,
    /// Deny execution on hook failure. Default for `Guardrail` and `Rewrite`.
    FailClosed,
}
```

### Session

Conversation state container:

```rust
pub struct Session {
    /// Unique session identifier
    pub id: SessionId,

    /// Model name
    pub model: String,

    /// Conversation messages
    pub messages: Vec<Message>,

    /// Cumulative token usage
    pub usage: Usage,

    /// Number of completed turns
    pub turns: u32,

    /// Session creation timestamp
    pub created_at: DateTime<Utc>,

    /// Last update timestamp
    pub updated_at: DateTime<Utc>,
}

impl Session {
    /// Create a new session
    pub fn new(model: impl Into<String>) -> Self

    /// Create with a specific ID
    pub fn with_id(id: SessionId, model: impl Into<String>) -> Self

    /// Add a message to the session
    pub fn add_message(&mut self, message: Message)

    /// Get the last assistant message
    pub fn last_assistant_message(&self) -> Option<&AssistantMessage>
}
```

### SessionId

Unique session identifier (UUIDv7):

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(String);

impl SessionId {
    /// Generate a new session ID
    pub fn new() -> Self

    /// Create from an existing string
    pub fn from_string(s: impl Into<String>) -> Self

    /// Get the string representation
    pub fn as_str(&self) -> &str
}

impl Display for SessionId { ... }
impl FromStr for SessionId { ... }
```

### Message

Conversation message types:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Message {
    /// System prompt
    System(SystemMessage),

    /// User input
    User(UserMessage),

    /// Assistant response
    Assistant(AssistantMessage),

    /// Tool execution results
    ToolResults { results: Vec<ToolResult> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SystemMessage {
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}
```

### ToolDef

Tool definition for LLM:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolDef {
    /// Tool name (must be unique)
    pub name: String,

    /// Human-readable description
    pub description: String,

    /// JSON Schema for input parameters
    pub input_schema: Value,
}
```

### ToolCall

Tool invocation request:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique call identifier
    pub id: String,

    /// Tool name
    pub name: String,

    /// Arguments as JSON
    pub args: Value,
}
```

### ToolResult

Tool execution result:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolResult {
    /// Corresponding tool call ID
    pub tool_call_id: String,

    /// Result content (success or error message)
    pub content: String,

    /// Whether the tool execution failed
    pub is_error: bool,
}

impl ToolResult {
    /// Create a success result
    pub fn success(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self

    /// Create an error result
    pub fn error(tool_call_id: impl Into<String>, message: impl Into<String>) -> Self
}
```

### Usage

Token usage tracking:

```rust
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Input tokens consumed
    pub input_tokens: u64,

    /// Output tokens generated
    pub output_tokens: u64,
}

impl Usage {
    /// Total tokens (input + output)
    pub fn total_tokens(&self) -> u64

    /// Add another usage to this one
    pub fn add(&mut self, other: &Usage)
}

impl std::ops::Add for Usage { ... }
impl std::ops::AddAssign for Usage { ... }
```

### StopReason

Why the LLM stopped generating:

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// Natural end of response
    EndTurn,

    /// Wants to call tools
    ToolUse,

    /// Hit token limit
    MaxTokens,

    /// Hit a stop sequence
    StopSequence,

    /// Content was filtered
    ContentFilter,

    /// User cancelled
    Cancelled,
}
```

### BudgetLimits

Resource constraints:

```rust
#[derive(Clone, Debug, Default)]
pub struct BudgetLimits {
    /// Maximum total tokens (input + output)
    pub max_tokens: Option<u64>,

    /// Maximum wall-clock duration
    pub max_duration: Option<Duration>,

    /// Maximum tool invocations
    pub max_tool_calls: Option<u32>,
}
```

### RetryPolicy

Error retry configuration:

```rust
#[derive(Clone, Debug)]
pub struct RetryPolicy {
    /// Maximum retry attempts
    pub max_retries: u32,

    /// Initial delay before first retry
    pub initial_delay: Duration,

    /// Maximum delay between retries
    pub max_delay: Duration,

    /// Exponential backoff multiplier
    pub multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
            multiplier: 2.0,
        }
    }
}
```

### RunResult

Agent execution result:

```rust
#[derive(Clone, Debug)]
pub struct RunResult {
    /// Final text response
    pub text: String,

    /// Session identifier
    pub session_id: SessionId,

    /// Number of turns executed
    pub turns: u32,

    /// Number of tool calls made
    pub tool_calls: u32,

    /// Total token usage
    pub usage: Usage,

    /// Structured output (if output_schema was provided)
    pub structured_output: Option<serde_json::Value>,

    /// Warnings from schema compilation (if any)
    pub schema_warnings: Option<Vec<SchemaWarning>>,
}
```

### AgentEvent

Streaming events during execution:

```rust
#[derive(Clone, Debug)]
pub enum AgentEvent {
    /// Agent run started
    RunStarted { session_id: SessionId },

    /// New turn started
    TurnStarted { turn: u32 },

    /// Text chunk received
    TextDelta { delta: String },

    /// Tool call requested by LLM
    ToolCallRequested { id: String, name: String },

    /// Tool result received
    ToolResultReceived { id: String, result: String },

    /// Turn completed
    TurnCompleted { turn: u32, usage: Usage },

    /// Run completed successfully
    RunCompleted { result: RunResult },

    /// Run failed with error
    RunFailed { error: String },

    /// Session checkpoint saved
    CheckpointSaved { session_id: SessionId },

    /// Hook lifecycle and audit events
    HookStarted { hook_id: String, point: HookPoint },
    HookCompleted { hook_id: String, point: HookPoint, duration_ms: u64 },
    HookFailed { hook_id: String, point: HookPoint, error: String },
    HookDenied {
        hook_id: String,
        point: HookPoint,
        reason_code: HookReasonCode,
        message: String,
        payload: Option<serde_json::Value>,
    },
    HookRewriteApplied { hook_id: String, point: HookPoint, patch: HookPatch },
    HookPatchPublished { hook_id: String, point: HookPoint, envelope: HookPatchEnvelope },

    /// Skill lifecycle events (emitted when skills feature is enabled)
    SkillsResolved { skills: Vec<SkillId>, injection_bytes: usize },
    SkillResolutionFailed { reference: String, error: String },
}
```

### AgentError

Error types:

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("LLM error: {0}")]
    Llm(#[from] LlmError),

    #[error("Tool error: {tool}: {message}")]
    Tool { tool: String, message: String },

    #[error("Session error: {0}")]
    Session(String),

    #[error("Budget exhausted: {0:?}")]
    BudgetExhausted(BudgetType),

    #[error("Cancelled")]
    Cancelled,

    #[error("Configuration error: {0}")]
    Config(String),

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

#[derive(Debug, Clone)]
pub enum BudgetType {
    Tokens,
    Duration,
    ToolCalls,
}
```

---

## Traits

### AgentLlmClient

LLM provider abstraction:

```rust
#[async_trait]
pub trait AgentLlmClient: Send + Sync {
    /// Stream a response from the LLM
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
    ) -> Result<LlmStreamResult, AgentError>;

    /// Get the provider name
    fn provider(&self) -> &'static str;
}

pub struct LlmStreamResult {
    pub stream: Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send>>,
}
```

### AgentToolDispatcher

Tool execution abstraction:

```rust
#[async_trait]
pub trait AgentToolDispatcher: Send + Sync {
    /// Get available tool definitions (Arc for zero-copy sharing)
    fn tools(&self) -> Arc<[Arc<ToolDef>]>;

    /// Execute a tool call
    ///
    /// `ToolCallView` is a zero-allocation borrowed view:
    ///   { id: &str, name: &str, args: &RawValue }
    /// Use `call.parse_args::<T>()` to deserialize arguments.
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError>;
}
```

### AgentSessionStore

Session persistence abstraction:

```rust
#[async_trait]
pub trait AgentSessionStore: Send + Sync {
    /// Save a session
    async fn save(&self, session: &Session) -> Result<(), AgentError>;

    /// Load a session by ID
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError>;

    /// List all sessions
    async fn list(&self) -> Result<Vec<SessionId>, AgentError>;
}
```

---

## Module: `meerkat_contracts`

Wire types, capability model, and error contracts shared across all protocol surfaces.

### CapabilityId

Every capability known to Meerkat. Adding a variant forces updates to the registry, error mappings, and codegen templates:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[derive(strum::EnumIter, strum::EnumString, strum::Display)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityId {
    Sessions,           // Core agent loop, session lifecycle
    Streaming,          // Event streaming during turns
    StructuredOutput,   // Schema-validated JSON output
    Hooks,              // Hook pipeline (always compiled, policy-controlled)
    Builtins,           // Task, wait, datetime tools (always compiled, policy-controlled)
    Shell,              // Shell execution tool (always compiled, policy-controlled)
    Comms,              // Inter-agent communication (feature: "comms")
    SubAgents,          // Sub-agent spawn/fork (feature: "sub-agents")
    MemoryStore,        // Semantic memory indexing (feature: "memory-store")
    SessionStore,       // Durable session persistence (feature: "session-store")
    SessionCompaction,  // Context compaction (feature: "session-compaction")
    Skills,             // Skill loading and injection (feature: "skills")
}
```

### CapabilityRegistration

Self-registration entry for capabilities. Feature-gated crates submit these via `inventory::submit!`. The `status_resolver` allows crates to check runtime config/policy to determine whether a capability is actually available (versus merely compiled in):

```rust
pub struct CapabilityRegistration {
    pub id: CapabilityId,
    pub description: &'static str,
    pub scope: CapabilityScope,
    pub requires_feature: Option<&'static str>,
    pub prerequisites: &'static [CapabilityId],
    pub status_resolver: Option<fn(&Config) -> CapabilityStatus>,
}
```

### CapabilityScope

Where a capability applies. Used in capability registrations to indicate whether a capability is available on all protocol surfaces or only specific ones:

```rust
pub enum CapabilityScope {
    /// Available on all protocol surfaces (CLI, REST, MCP Server, JSON-RPC).
    Universal,
    /// Available only on specific protocols.
    Extension { protocols: Cow<'static, [Protocol]> },
}
```

### Protocol

Protocol surface identifiers used in capability scoping and error projections:

```rust
pub enum Protocol {
    Rpc,
    Rest,
    Mcp,
    Cli,
}
```

### CapabilityStatus

Runtime status of a capability after build, config, and protocol resolution:

```rust
pub enum CapabilityStatus {
    /// Compiled in, config-enabled, protocol supports it.
    Available,

    /// Compiled in but disabled by policy. The description is human-readable
    /// because the resolution chain (factory flags -> build config -> tool policy)
    /// cannot be captured by a single config path.
    DisabledByPolicy { description: Cow<'static, str> },

    /// Not compiled into this build (feature flag absent).
    NotCompiled { feature: Cow<'static, str> },

    /// This protocol surface doesn't support it.
    NotSupportedByProtocol { reason: Cow<'static, str> },
}
```

### ErrorCode

Stable error codes for wire protocol, with projections to JSON-RPC codes, HTTP status, and CLI exit codes:

```rust
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    SessionNotFound,        // RPC: -32001, HTTP: 404, CLI: 10
    SessionBusy,            // RPC: -32002, HTTP: 409, CLI: 11
    SessionNotRunning,      // RPC: -32003, HTTP: 409, CLI: 12
    ProviderError,          // RPC: -32010, HTTP: 502, CLI: 20
    BudgetExhausted,        // RPC: -32011, HTTP: 429, CLI: 21
    HookDenied,             // RPC: -32012, HTTP: 403, CLI: 22
    AgentError,             // RPC: -32013, HTTP: 500, CLI: 30
    CapabilityUnavailable,  // RPC: -32020, HTTP: 501, CLI: 40
    SkillNotFound,          // RPC: -32021, HTTP: 404, CLI: 41
    SkillResolutionFailed,  // RPC: -32022, HTTP: 422, CLI: 42
    InvalidParams,          // RPC: -32602, HTTP: 400, CLI: 2
    InternalError,          // RPC: -32603, HTTP: 500, CLI: 1
}

impl ErrorCode {
    pub const fn jsonrpc_code(self) -> i32;
    pub const fn http_status(self) -> u16;
    pub const fn cli_exit_code(self) -> i32;
    pub fn category(self) -> ErrorCategory;
}
```

### WireError

Canonical error envelope used by all surfaces:

```rust
pub struct WireError {
    pub code: ErrorCode,
    pub category: ErrorCategory,
    pub message: Cow<'static, str>,
    pub details: Option<serde_json::Value>,
    pub capability_hint: Option<CapabilityHint>,
}

impl WireError {
    pub fn new(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self;
    pub fn with_capability_hint(self, hint: CapabilityHint) -> Self;
    pub fn with_details(self, details: serde_json::Value) -> Self;
}

impl From<SessionError> for WireError { ... }
```

`CapabilityHint` points the caller to the missing capability when `CapabilityUnavailable` is returned:

```rust
pub struct CapabilityHint {
    pub capability_id: CapabilityId,
    pub message: Cow<'static, str>,
}
```

### ContractVersion

Semver-style version embedded in events, capabilities response, SDK packages, and RPC handshake:

```rust
pub struct ContractVersion { major: u32, minor: u32, patch: u32 }

impl ContractVersion {
    pub const CURRENT: Self;  // Currently 0.1.0
    pub fn is_compatible_with(&self, other: &Self) -> bool;
}
// + Display, FromStr, Copy, Ord
```

### Wire Request Fragments

Composable request parameter groups. Protocol crates inline the fields they support and provide accessor methods returning the fragment type. No `#[serde(flatten)]`:

```rust
/// Core session creation parameters.
pub struct CoreCreateParams {
    pub prompt: String,
    pub model: Option<String>,
    pub provider: Option<Provider>,
    pub max_tokens: Option<u32>,
    pub system_prompt: Option<String>,
}

/// Structured output parameters.
pub struct StructuredOutputParams {
    pub output_schema: Option<OutputSchema>,
    pub structured_output_retries: Option<u32>,
}

/// Comms parameters.
pub struct CommsParams {
    pub host_mode: bool,
    pub comms_name: Option<String>,
}

/// Hook parameters.
pub struct HookParams {
    pub hooks_override: Option<HookRunOverrides>,
}

/// Skills parameters.
pub struct SkillsParams {
    pub skills_enabled: bool,
    pub skill_references: Vec<String>,
}
```

### OutputSchema

Schema definition for structured output. Defined in `meerkat_core::types` and re-exported by the facade. Used in `StructuredOutputParams` and `RunResult`. Accepts both raw JSON schemas and wrapped schemas (with `name`, `strict`, `compat`, `format` fields). Providers receive a lowered schema suited to their API (Anthropic, OpenAI, Gemini).

```rust
pub struct OutputSchema {
    /// The JSON schema that the output must conform to.
    /// Normalized on construction (object types get `properties` and `required` keys).
    pub schema: MeerkatSchema,
    /// Optional name for the schema (used by some providers, e.g., OpenAI function calling).
    pub name: Option<String>,
    /// Strict mode: when true, the agent retries if the LLM output fails validation
    /// against the schema. The number of retries is controlled by `structured_output_retries`.
    pub strict: bool,
    /// Provider compatibility mode for schema lowering.
    pub compat: SchemaCompat,
    /// Schema format version.
    pub format: SchemaFormat,
}

impl OutputSchema {
    pub fn new(schema: Value) -> Result<Self, SchemaError>;
    pub fn with_name(self, name: impl Into<String>) -> Self;
    pub fn strict(self) -> Self;
    pub fn with_compat(self, compat: SchemaCompat) -> Self;
    pub fn from_json_str(raw: &str) -> Result<Self, SchemaError>;
    pub fn from_json_value(value: Value) -> Result<Self, SchemaError>;
    /// Create from a Rust type via schemars (compile-time schema generation).
    pub fn from_type<T: schemars::JsonSchema>() -> Result<Self, SchemaError>;
}
```

### MeerkatSchema

Wrapper around a JSON `Value` that enforces normalization (object types always have `properties` and `required` keys). Created via `MeerkatSchema::new(value)` which returns `Err(SchemaError::InvalidRoot)` if the root is not a JSON object.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MeerkatSchema(Value);

impl MeerkatSchema {
    pub fn new(schema: Value) -> Result<Self, SchemaError>;
    pub fn as_value(&self) -> &Value;
}
```

### SchemaCompat

Controls how the schema is lowered for providers that have schema restrictions:

```rust
pub enum SchemaCompat {
    /// Allow lossy lowering (drop unsupported features silently). Default.
    Lossy,
    /// Reject schemas that cannot be represented exactly by the provider.
    Strict,
}
```

### SchemaFormat

Schema format version. Currently only one version exists:

```rust
pub enum SchemaFormat {
    /// Meerkat V1 format. Default.
    MeerkatV1,
}
```

### SchemaWarning

Emitted during provider-specific schema lowering when features are dropped or adapted:

```rust
pub struct SchemaWarning {
    pub provider: Provider,
    pub path: String,
    pub message: String,
}
```

---

## Module: `meerkat_core::skills`

Skill system contracts. The `meerkat-skills` crate provides implementations.

### SkillId

Newtype identifier for type safety:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SkillId(pub String);

impl Display for SkillId { ... }
```

### SkillScope

Where a skill was discovered:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    Builtin,   // Embedded in a component crate via inventory
    Project,   // .rkat/skills/
    User,      // ~/.rkat/skills/
}
```

### SkillDescriptor

Metadata describing a skill:

```rust
pub struct SkillDescriptor {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    /// Capability IDs required for this skill (as string forms of CapabilityId).
    /// Using strings avoids circular dependency between meerkat-core and
    /// meerkat-contracts. Resolved to typed IDs at runtime by meerkat-skills.
    pub requires_capabilities: Vec<String>,
}
```

### SkillDocument

A loaded skill with its full content:

```rust
pub struct SkillDocument {
    pub descriptor: SkillDescriptor,
    pub body: String,
    pub extensions: IndexMap<String, String>,
}
```

### SkillError

```rust
pub enum SkillError {
    NotFound { id: SkillId },
    CapabilityUnavailable { id: SkillId, capability: String },
    Ambiguous { reference: String, matches: Vec<SkillId> },
    Load(Cow<'static, str>),
    Parse(Cow<'static, str>),
}
```

### SkillSource (trait)

```rust
#[async_trait]
pub trait SkillSource: Send + Sync {
    async fn list(&self) -> Result<Vec<SkillDescriptor>, SkillError>;
    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError>;
}
```

### SkillEngine (trait)

```rust
#[async_trait]
pub trait SkillEngine: Send + Sync {
    /// Generate the skill inventory section for the system prompt.
    async fn inventory_section(&self) -> Result<String, SkillError>;
    /// Resolve skill references and render injection content.
    async fn resolve_and_render(
        &self,
        references: &[String],
        available_capabilities: &[String],
    ) -> Result<String, SkillError>;
}
```

---

## Module: `meerkat_client`

LLM provider implementations.

### LlmClient

Low-level client trait:

```rust
pub trait LlmClient: Send + Sync {
    /// Stream response events
    fn stream(&self, request: &LlmRequest) -> Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send>>;

    /// Get provider name
    fn provider(&self) -> &'static str;
}
```

### LlmRequest

Request to LLM:

```rust
pub struct LlmRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub max_tokens: u32,
    pub system: Option<String>,
}
```

### LlmEvent

Streaming events from LLM:

```rust
pub enum LlmEvent {
    /// Text chunk
    TextDelta { delta: String },

    /// Partial tool call
    ToolCallDelta { id: String, name: Option<String>, args_delta: String },

    /// Complete tool call
    ToolCallComplete { id: String, name: String, args: Value },

    /// Usage update
    UsageUpdate { usage: Usage },

    /// Stream complete
    Done { stop_reason: StopReason },
}
```

### LlmError

LLM-specific errors:

```rust
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("Rate limited, retry after {retry_after:?}")]
    RateLimited { retry_after: Option<Duration> },

    #[error("Server overloaded")]
    ServerOverloaded,

    #[error("Network timeout")]
    NetworkTimeout,

    #[error("Connection reset")]
    ConnectionReset,

    #[error("Server error {status}: {message}")]
    ServerError { status: u16, message: String },

    #[error("Invalid request: {message}")]
    InvalidRequest { message: String },

    #[error("Authentication failed")]
    AuthenticationFailed,

    #[error("Content filtered")]
    ContentFiltered,

    #[error("Context length exceeded")]
    ContextLengthExceeded,

    #[error("Model not found: {model}")]
    ModelNotFound { model: String },

    #[error("Invalid API key")]
    InvalidApiKey,

    #[error("Unknown error: {message}")]
    Unknown { message: String },
}

impl LlmError {
    /// Whether this error can be retried
    pub fn is_retryable(&self) -> bool
}
```

### AnthropicClient

Anthropic Claude implementation:

```rust
pub struct AnthropicClient {
    // Private fields
}

impl AnthropicClient {
    /// Create with API key
    pub fn new(api_key: impl Into<String>) -> Self

    /// Create with custom base URL
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self
}

impl LlmClient for AnthropicClient { ... }
```

### OpenAiClient

OpenAI GPT implementation:

```rust
pub struct OpenAiClient {
    // Private fields
}

impl OpenAiClient {
    /// Create with API key
    pub fn new(api_key: impl Into<String>) -> Self

    /// Create with custom base URL (for Azure, etc.)
    pub fn with_base_url(api_key: impl Into<String>, base_url: impl Into<String>) -> Self
}

impl LlmClient for OpenAiClient { ... }
```

### GeminiClient

Google Gemini implementation:

```rust
pub struct GeminiClient {
    // Private fields
}

impl GeminiClient {
    /// Create with API key
    pub fn new(api_key: impl Into<String>) -> Self
}

impl LlmClient for GeminiClient { ... }
```

---

## Module: `meerkat_store`

Session persistence implementations.

### JsonlStore

JSONL file-based storage:

```rust
pub struct JsonlStore {
    // Private fields
}

impl JsonlStore {
    /// Create with storage directory
    pub fn new(path: impl Into<PathBuf>) -> Self

    /// Initialize the store (create directory if needed)
    pub async fn init(&self) -> Result<(), AgentError>
}

impl AgentSessionStore for JsonlStore { ... }
```

### MemoryStore

In-memory storage (for testing):

```rust
pub struct MemoryStore {
    // Private fields
}

impl MemoryStore {
    /// Create empty store
    pub fn new() -> Self
}

impl AgentSessionStore for MemoryStore { ... }
```

---

## Module: `meerkat_mcp_client`

MCP protocol client.

### McpConnection

Connection to an MCP server:

```rust
pub struct McpConnection {
    // Private fields
}

impl McpConnection {
    /// Connect to an MCP server
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpError>

    /// List available tools
    pub async fn list_tools(&self) -> Result<Vec<ToolDef>, McpError>

    /// Call a tool
    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<String, McpError>

    /// Close the connection
    pub async fn close(&self) -> Result<(), McpError>
}
```

### McpRouter

Routes tool calls to multiple MCP servers:

```rust
pub struct McpRouter {
    // Private fields
}

impl McpRouter {
    /// Create empty router
    pub fn new() -> Self

    /// Add an MCP server
    pub async fn add_server(&self, config: McpServerConfig) -> Result<(), McpError>

    /// Remove an MCP server
    pub async fn remove_server(&self, name: &str) -> Result<(), McpError>

    /// List all tools from all servers
    pub async fn list_tools(&self) -> Result<Vec<ToolDef>, McpError>
}

impl AgentToolDispatcher for McpRouter { ... }
```

### McpServerConfig

MCP server configuration:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Server name (for routing)
    pub name: String,

    /// Command to execute
    pub command: String,

    /// Command arguments
    pub args: Vec<String>,

    /// Environment variables
    pub env: HashMap<String, String>,
}
```

### McpError

MCP-specific errors:

```rust
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Tool not found: {0}")]
    ToolNotFound(String),

    #[error("Server not found: {0}")]
    ServerNotFound(String),

    #[error("Timeout")]
    Timeout,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

---

## Module: `meerkat_tools`

Tool registry and validation.

### ToolRegistry

Manage tool definitions:

```rust
pub struct ToolRegistry {
    // Private fields
}

impl ToolRegistry {
    /// Create empty registry
    pub fn new() -> Self

    /// Register a tool
    pub fn register(&mut self, tool: ToolDef) -> Result<(), ToolError>

    /// Unregister a tool
    pub fn unregister(&mut self, name: &str) -> bool

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<&ToolDef>

    /// Get all tools
    pub fn tools(&self) -> Vec<ToolDef>

    /// Validate arguments against schema
    pub fn validate(&self, name: &str, args: &Value) -> Result<(), ToolError>
}
```

### ToolDispatcher

Execute registered tools:

```rust
pub struct ToolDispatcher<F> {
    // Private fields
}

impl<F> ToolDispatcher<F>
where
    F: Fn(&str, &Value) -> BoxFuture<'static, Result<String, String>> + Send + Sync,
{
    /// Create with handler function
    pub fn new(handler: F) -> Self

    /// Add tool definitions
    pub fn with_tools(self, tools: Vec<ToolDef>) -> Self
}

impl<F> AgentToolDispatcher for ToolDispatcher<F> { ... }
```

---

## Sub-Agent Operations

### Fork

Create parallel branches with full context:

```rust
pub struct ForkBranch {
    /// Branch identifier
    pub id: String,

    /// Prompt for this branch
    pub prompt: String,
}

impl Agent {
    /// Fork into parallel branches
    pub async fn fork(&mut self, branches: Vec<ForkBranch>) -> Result<Vec<RunResult>, AgentError>
}
```

### Spawn

Create child agents with limited context:

```rust
pub struct SpawnSpec {
    /// Child identifier
    pub id: String,

    /// Prompt for the child
    pub prompt: String,

    /// How much context to provide
    pub context_strategy: ContextStrategy,

    /// Which tools the child can use
    pub tool_access: ToolAccessPolicy,

    /// Budget allocation
    pub budget_policy: ForkBudgetPolicy,
}

pub enum ContextStrategy {
    /// Copy entire conversation
    FullHistory,

    /// Copy last N turns
    LastTurns(usize),

    /// Summarize to fit token budget
    Summary { max_tokens: u32 },

    /// Provide explicit messages
    Custom { messages: Vec<Message> },
}

pub enum ToolAccessPolicy {
    /// Access all tools
    All,

    /// Only these tools
    AllowList(Vec<String>),

    /// All except these
    DenyList(Vec<String>),

    /// No tool access
    None,
}

pub enum ForkBudgetPolicy {
    /// Share parent's remaining budget
    Shared,

    /// Fixed token allocation
    Fixed(u64),

    /// Percentage of parent's budget
    Percentage(f64),
}

impl Agent {
    /// Spawn a child agent
    pub async fn spawn(&mut self, spec: SpawnSpec) -> Result<RunResult, AgentError>
}
```

---

## CLI Commands

### rkat run

```
rkat run [OPTIONS] <PROMPT>

Arguments:
  <PROMPT>  The prompt to send to the agent

Options:
  -m, --model <MODEL>          Model to use [default: claude-sonnet-4]
  -t, --max-tokens <N>         Maximum tokens per turn [default: 4096]
      --max-duration <DUR>     Maximum runtime (e.g., "30s", "5m")
      --max-tool-calls <N>     Maximum tool invocations
  -s, --system-prompt <TEXT>   System prompt
  -o, --output <FORMAT>        Output format: text, json [default: text]
      --stream                 Stream output as it arrives
  -h, --help                   Print help
```

### rkat resume

```
rkat resume [OPTIONS] <SESSION_ID> <PROMPT>

Arguments:
  <SESSION_ID>  Session ID to resume
  <PROMPT>      The prompt to send

Options:
  -m, --model <MODEL>      Model to use [default: claude-sonnet-4]
  -t, --max-tokens <N>     Maximum tokens [default: 4096]
  -o, --output <FORMAT>    Output format [default: text]
  -h, --help               Print help
```

### rkat sessions

```
rkat sessions <COMMAND>

Commands:
  list   List recent sessions
  show   Show session details

rkat sessions list [OPTIONS]
Options:
  --limit <N>  Maximum sessions to list [default: 10]

rkat sessions show <SESSION_ID>
```

---

## REST API Endpoints

### POST /sessions

Create and run a new session.

Request:
```json
{
  "prompt": "Your prompt here",
  "model": "claude-sonnet-4",
  "system_prompt": "Optional system prompt",
  "max_tokens": 4096
}
```

Response:
```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000001",
  "text": "Response text",
  "turns": 1,
  "tool_calls": 0,
  "usage": {
    "input_tokens": 50,
    "output_tokens": 200
  },
  "stop_reason": "end_turn"
}
```

### POST /sessions/{id}/messages

Continue an existing session.

Request:
```json
{
  "prompt": "Follow-up message"
}
```

Response: Same as POST /sessions

### GET /sessions/{id}

Get session details.

Response:
```json
{
  "id": "01936f8a-7b2c-7000-8000-000000000001",
  "model": "claude-sonnet-4",
  "turns": 3,
  "usage": {
    "input_tokens": 500,
    "output_tokens": 1200
  },
  "created_at": "2024-01-15T10:30:00Z",
  "updated_at": "2024-01-15T10:35:00Z"
}
```

### GET /sessions/{id}/events

Server-Sent Events stream for real-time updates.

Event types:
- `run_started`
- `turn_started`
- `text_delta`
- `tool_call_requested`
- `tool_result_received`
- `turn_completed`
- `run_completed`
- `run_failed`

---

## MCP Server Tools

When running as an MCP server, Meerkat exposes these tools:

### meerkat_run

Run a new agent with a prompt.

Input:
```json
{
  "prompt": "Your prompt",
  "model": "claude-sonnet-4",
  "system_prompt": "Optional",
  "max_tokens": 4096
}
```

### meerkat_resume

Resume an existing session.

Input:
```json
{
  "session_id": "01936f8a-7b2c-7000-8000-000000000001",
  "prompt": "Continue with this"
}
```
