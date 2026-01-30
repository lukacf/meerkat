//! Agent - the core agent orchestrator
//!
//! The Agent struct ties together all components and runs the agent loop.

mod builder;
mod comms;
mod runner;
mod state;

use crate::budget::Budget;
use crate::comms_runtime::CommsRuntime;
use crate::config::AgentConfig;
use crate::error::AgentError;
use crate::retry::RetryPolicy;
use crate::session::Session;
use crate::state::LoopState;
use crate::sub_agent::SubAgentManager;
use crate::types::{Message, StopReason, ToolCall, ToolDef, Usage};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::mpsc;

pub use builder::AgentBuilder;
pub use runner::AgentRunner;

/// Special error prefix to signal tool calls that must be routed externally.
pub const CALLBACK_TOOL_PREFIX: &str = "CALLBACK_TOOL_PENDING:";

/// Trait for LLM clients that can be used with the agent
#[async_trait]
pub trait AgentLlmClient: Send + Sync {
    /// Stream a response from the LLM
    ///
    /// # Arguments
    /// * `messages` - Conversation history
    /// * `tools` - Available tool definitions
    /// * `max_tokens` - Maximum tokens to generate
    /// * `temperature` - Sampling temperature
    /// * `provider_params` - Provider-specific parameters (e.g., thinking config)
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError>;

    /// Get the provider name
    fn provider(&self) -> &'static str;
}

/// Result of streaming from the LLM
pub struct LlmStreamResult {
    content: String,
    tool_calls: Vec<ToolCall>,
    stop_reason: StopReason,
    usage: Usage,
}

impl LlmStreamResult {
    pub fn new(
        content: String,
        tool_calls: Vec<ToolCall>,
        stop_reason: StopReason,
        usage: Usage,
    ) -> Self {
        Self {
            content,
            tool_calls,
            stop_reason,
            usage,
        }
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn tool_calls(&self) -> &[ToolCall] {
        &self.tool_calls
    }

    pub fn stop_reason(&self) -> StopReason {
        self.stop_reason
    }

    pub fn usage(&self) -> &Usage {
        &self.usage
    }

    pub fn into_parts(self) -> (String, Vec<ToolCall>, StopReason, Usage) {
        (self.content, self.tool_calls, self.stop_reason, self.usage)
    }
}

/// Trait for tool dispatchers
#[async_trait]
pub trait AgentToolDispatcher: Send + Sync {
    /// Get available tool definitions
    fn tools(&self) -> Arc<[Arc<ToolDef>]>;

    /// Execute a tool call
    ///
    /// Returns the tool result as a JSON Value on success, or a ToolError on failure.
    /// The Value will be stringified when creating ToolResult.content for the LLM.
    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, crate::error::ToolError>;
}

/// A tool dispatcher that filters tools based on a policy
pub struct FilteredToolDispatcher<T: AgentToolDispatcher + ?Sized> {
    inner: Arc<T>,
    /// HashSet for O(1) lookup instead of Vec O(n)
    allowed_tools: HashSet<String>,
    cache: RwLock<FilteredToolCache>,
}

impl<T: AgentToolDispatcher + ?Sized> FilteredToolDispatcher<T> {
    pub fn new(inner: Arc<T>, allowed_tools: Vec<String>) -> Self {
        Self {
            inner,
            allowed_tools: allowed_tools.into_iter().collect(),
            cache: RwLock::new(FilteredToolCache::default()),
        }
    }
}

#[derive(Debug)]
struct FilteredToolCache {
    inner_ptr: usize,
    inner_len: usize,
    tools: Arc<[Arc<ToolDef>]>,
    valid: bool,
}

impl Default for FilteredToolCache {
    fn default() -> Self {
        Self {
            inner_ptr: 0,
            inner_len: 0,
            tools: Arc::from([]),
            valid: false,
        }
    }
}

#[async_trait]
impl<T: AgentToolDispatcher + ?Sized + 'static> AgentToolDispatcher for FilteredToolDispatcher<T> {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        let inner = self.inner.tools();
        let inner_ptr = inner.as_ptr() as usize;
        let inner_len = inner.len();

        if let Ok(cached) = self.cache.try_read() {
            if cached.valid && cached.inner_ptr == inner_ptr && cached.inner_len == inner_len {
                return Arc::clone(&cached.tools);
            }
        }

        let filtered: Vec<Arc<ToolDef>> = inner
            .iter()
            .filter(|t| self.allowed_tools.contains(t.name.as_str()))
            .map(Arc::clone)
            .collect();
        let filtered: Arc<[Arc<ToolDef>]> = filtered.into();

        if let Ok(mut cached) = self.cache.try_write() {
            cached.inner_ptr = inner_ptr;
            cached.inner_len = inner_len;
            cached.tools = Arc::clone(&filtered);
            cached.valid = true;
        }

        filtered
    }

    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, crate::error::ToolError> {
        if !self.allowed_tools.contains(name) {
            return Err(crate::error::ToolError::access_denied(name));
        }
        self.inner.dispatch(name, args).await
    }
}

/// Trait for session stores
#[async_trait]
pub trait AgentSessionStore: Send + Sync {
    /// Save a session
    async fn save(&self, session: &Session) -> Result<(), AgentError>;

    /// Load a session by ID
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError>;
}

/// The main Agent struct
///
/// Supports both concrete types and trait objects (`dyn Trait`).
/// When using trait objects, pass `Arc<dyn AgentLlmClient>` etc.
pub struct Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized,
    T: AgentToolDispatcher + ?Sized,
    S: AgentSessionStore + ?Sized,
{
    config: AgentConfig,
    client: Arc<C>,
    tools: Arc<T>,
    store: Arc<S>,
    session: Session,
    budget: Budget,
    retry_policy: RetryPolicy,
    state: LoopState,
    sub_agent_manager: Arc<SubAgentManager>,
    depth: u32,
    steering_rx: mpsc::Receiver<crate::ops::SteeringMessage>,
    steering_tx: mpsc::Sender<crate::ops::SteeringMessage>,
    /// Optional comms runtime for inter-agent communication.
    /// None if comms is disabled or if this is a subagent (subagents cannot have comms).
    comms_runtime: Option<CommsRuntime>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::budget::BudgetLimits;
    use std::sync::Mutex;

    fn empty_object_schema() -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert("type".to_string(), Value::String("object".to_string()));
        obj.insert(
            "properties".to_string(),
            Value::Object(serde_json::Map::new()),
        );
        obj.insert("required".to_string(), Value::Array(Vec::new()));
        Value::Object(obj)
    }

    // Mock LLM client for testing
    struct MockLlmClient {
        responses: Mutex<Vec<LlmStreamResult>>,
    }

    impl MockLlmClient {
        fn new(responses: Vec<LlmStreamResult>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    #[async_trait]
    impl AgentLlmClient for MockLlmClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&Value>,
        ) -> Result<LlmStreamResult, AgentError> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(LlmStreamResult::new(
                    "Default response".to_string(),
                    vec![],
                    StopReason::EndTurn,
                    Usage::default(),
                ))
            } else {
                Ok(responses.remove(0))
            }
        }

        fn provider(&self) -> &'static str {
            "mock"
        }
    }

    // Mock tool dispatcher
    struct MockToolDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    impl MockToolDispatcher {
        fn new(tools: Vec<Arc<ToolDef>>) -> Self {
            Self {
                tools: tools.into(),
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for MockToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.tools.clone()
        }

        async fn dispatch(
            &self,
            _name: &str,
            _args: &Value,
        ) -> Result<Value, crate::error::ToolError> {
            Ok(Value::Null)
        }
    }

    // Mock session store
    struct MockSessionStore;

    #[async_trait]
    impl AgentSessionStore for MockSessionStore {
        async fn save(&self, _session: &Session) -> Result<(), AgentError> {
            Ok(())
        }

        async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn test_agent_simple_response() {
        let client = Arc::new(MockLlmClient::new(vec![LlmStreamResult {
            content: "Hello! I'm an AI assistant.".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 10,
                output_tokens: 20,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        }]));

        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .system_prompt("You are a helpful assistant.")
            .build(client, tools, store)
            .await;

        let result = agent.run("Hello".to_string()).await.unwrap();

        assert_eq!(result.text, "Hello! I'm an AI assistant.");
        assert_eq!(result.turns, 1);
        assert_eq!(result.usage.input_tokens, 10);
        assert_eq!(result.usage.output_tokens, 20);
    }

    #[tokio::test]
    async fn test_agent_with_tool_call() {
        let client = Arc::new(MockLlmClient::new(vec![
            // First response: tool call
            LlmStreamResult {
                content: "Let me get the weather.".to_string(),
                tool_calls: vec![ToolCall::new(
                    "tc_1".to_string(),
                    "get_weather".to_string(),
                    serde_json::json!({"city": "Tokyo"}),
                )],
                stop_reason: StopReason::ToolUse,
                usage: Usage {
                    input_tokens: 10,
                    output_tokens: 15,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            },
            // Second response: final answer
            LlmStreamResult {
                content: "The weather in Tokyo is sunny.".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage {
                    input_tokens: 25,
                    output_tokens: 20,
                    cache_creation_tokens: None,
                    cache_read_tokens: None,
                },
            },
        ]));

        let tools = Arc::new(MockToolDispatcher::new(vec![Arc::new(ToolDef {
            name: "get_weather".to_string(),
            description: "Get weather for a city".to_string(),
            input_schema: empty_object_schema(),
        })]));

        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools, store)
            .await;

        let result = agent
            .run("What's the weather in Tokyo?".to_string())
            .await
            .unwrap();

        assert_eq!(result.text, "The weather in Tokyo is sunny.");
        assert_eq!(result.turns, 2);
        assert_eq!(result.tool_calls, 1);
        // Total usage should include both turns
        assert_eq!(result.usage.input_tokens, 35);
        assert_eq!(result.usage.output_tokens, 35);
    }

    #[tokio::test]
    async fn test_agent_builder() {
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new()
            .model("claude-3")
            .system_prompt("Test prompt")
            .max_tokens_per_turn(1000)
            .temperature(0.7)
            .budget(BudgetLimits {
                max_tokens: Some(10000),
                max_duration: None,
                max_tool_calls: Some(5),
            })
            .build(client, tools, store)
            .await;

        assert!(!agent.session().messages().is_empty()); // Should have system prompt
        assert_eq!(agent.state(), &LoopState::CallingLlm);
    }

    // =========================================================================
    // Phase 10: Agent Integration Tests (comms wiring)
    // =========================================================================

    #[tokio::test]
    async fn test_agent_builder_has_comms_config() {
        // Verify AgentBuilder has comms_config field
        let builder = AgentBuilder::new();
        // The field exists (compiles) and is None by default
        // We test this by using the comms() method
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = builder.build(client, tools, store).await;
        // Without comms config, comms runtime should be None
        assert!(agent.comms().is_none());
    }

    #[tokio::test]
    async fn test_agent_builder_comms_method() {
        use crate::comms_config::CoreCommsConfig;

        let config = CoreCommsConfig::with_name("test-agent");
        let builder = AgentBuilder::new().comms(config.clone());

        // Verify builder accepted the config (implicitly - build will use it)
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Note: This test doesn't verify comms_runtime creation because that
        // requires filesystem access. See test_agent_builder_creates_comms_runtime.
        let _agent = builder.build(client, tools, store).await;
    }

    #[tokio::test]
    async fn test_agent_has_comms_runtime() {
        // Verify Agent has comms_runtime field (accessible via comms())
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new().build(client, tools, store).await;

        // comms() and comms_mut() methods should exist and return None when disabled
        assert!(agent.comms().is_none());
    }

    #[tokio::test]
    async fn test_agent_builder_creates_comms_runtime() {
        use crate::comms_config::CoreCommsConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        // No listeners to avoid port binding issues in tests
        config.listen_uds = None;
        config.listen_tcp = None;

        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .build(client, tools, store)
            .await;

        // Comms runtime should be created
        assert!(agent.comms().is_some());

        // Public key should be valid
        let pubkey = agent.comms().unwrap().public_key();
        assert_eq!(pubkey.as_bytes().len(), 32);
    }

    #[tokio::test]
    async fn test_agent_comms_accessor() {
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new().build(client, tools, store).await;

        // comms() should return Option<&CommsRuntime>
        let comms_ref: Option<&crate::comms_runtime::CommsRuntime> = agent.comms();
        assert!(comms_ref.is_none());
    }

    #[tokio::test]
    async fn test_agent_comms_mut_accessor() {
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new().build(client, tools, store).await;

        // comms_mut() should return Option<&mut CommsRuntime>
        let comms_mut: Option<&mut crate::comms_runtime::CommsRuntime> = agent.comms_mut();
        assert!(comms_mut.is_none());
    }

    #[tokio::test]
    async fn test_subagent_has_no_comms() {
        use crate::comms_config::CoreCommsConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");

        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Build agent with depth > 0 (simulating subagent)
        let agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .depth(1) // Subagent depth
            .build(client, tools, store)
            .await;

        // Subagents cannot have comms - security restriction
        assert!(agent.comms().is_none());
        assert_eq!(agent.depth(), 1);
    }

    #[tokio::test]
    async fn test_agent_with_comms_runtime() {
        use crate::comms_config::CoreCommsConfig;
        use crate::comms_runtime::CommsRuntime;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        config.listen_uds = None;
        config.listen_tcp = None;

        // Create CommsRuntime externally
        let resolved = config.resolve_paths(tmp.path());
        let runtime = CommsRuntime::new(resolved).await.unwrap();

        // Get Arc clones before moving runtime
        let router_arc = runtime.router_arc();
        let trusted_peers_shared = runtime.trusted_peers_shared();

        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Inject the pre-created runtime
        let agent = AgentBuilder::new()
            .with_comms_runtime(runtime)
            .build(client, tools, store)
            .await;

        // Agent should have comms
        assert!(agent.comms().is_some());

        // The router should be the same as what we extracted
        assert!(std::ptr::eq(
            agent.comms().unwrap().router() as *const _,
            router_arc.as_ref() as *const _
        ));

        // The trusted_peers should point to the same underlying data (shared Arc<RwLock<...>>)
        // We verify this by checking that the Arc pointers are the same
        assert!(std::sync::Arc::ptr_eq(
            &agent.comms().unwrap().trusted_peers_shared(),
            &trusted_peers_shared
        ));
    }

    #[tokio::test]
    async fn test_agent_no_comms_tools_when_disabled() {
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![Arc::new(ToolDef {
            name: "my_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: empty_object_schema(),
        })]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new().build(client, tools, store).await;

        // Without comms, agent should have no comms runtime
        assert!(agent.comms().is_none());
    }

    #[tokio::test]
    async fn test_agent_empty_inbox_nonblocking() {
        use crate::comms_config::CoreCommsConfig;
        use std::time::{Duration, Instant};
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        config.listen_uds = None;
        config.listen_tcp = None;

        let client = Arc::new(MockLlmClient::new(vec![LlmStreamResult {
            content: "Done".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .build(client, tools, store)
            .await;

        // Verify comms is enabled
        assert!(agent.comms().is_some());

        // Run agent - should complete quickly even with empty inbox
        let start = Instant::now();
        let result = agent.run("Test".to_string()).await.unwrap();
        let elapsed = start.elapsed();

        // Should complete in reasonable time (not blocked on inbox)
        assert!(
            elapsed < Duration::from_secs(5),
            "Agent run took too long: {:?}",
            elapsed
        );
        assert_eq!(result.text, "Done");
    }

    #[tokio::test]
    async fn test_agent_starts_comms_listeners() {
        // Test that listeners are started automatically when comms is enabled.
        // We verify this by checking that the runtime was created (listeners start
        // during CommsRuntime::start_listeners() which is called in build()).
        use crate::comms_config::CoreCommsConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        // Enable UDS listener
        config.listen_uds = Some(tmp.path().join("test.sock"));
        config.listen_tcp = None;

        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let agent = AgentBuilder::new()
            .comms(config.clone())
            .comms_base_dir(tmp.path().to_path_buf())
            .build(client, tools, store)
            .await;

        // Comms runtime should exist
        assert!(agent.comms().is_some());

        // Give the listener a moment to create the socket
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Socket file should have been created by the listener
        // (This verifies start_listeners was called)
        assert!(
            config.listen_uds.as_ref().unwrap().exists(),
            "UDS socket file should exist after listener starts"
        );
    }

    #[tokio::test]
    async fn test_agent_drains_inbox_at_turn_boundary() {
        // This test verifies that drain_comms_inbox is called at turn boundaries.
        // Since we can't easily inject messages into the inbox, we verify:
        // 1. The method exists and is called (via code inspection)
        // 2. Empty inbox doesn't affect agent behavior
        // Full inbox draining is tested in e2e tests with real connections.
        use crate::comms_config::CoreCommsConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        config.listen_uds = None;
        config.listen_tcp = None;

        let client = Arc::new(MockLlmClient::new(vec![
            // First: tool call response
            LlmStreamResult {
                content: "Calling tool".to_string(),
                tool_calls: vec![ToolCall::new(
                    "tc_1".to_string(),
                    "test_tool".to_string(),
                    serde_json::json!({}),
                )],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            // Second: final response (after turn boundary where inbox would be drained)
            LlmStreamResult {
                content: "Done after turn boundary".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));
        let tools = Arc::new(MockToolDispatcher::new(vec![Arc::new(ToolDef {
            name: "test_tool".to_string(),
            description: "Test".to_string(),
            input_schema: empty_object_schema(),
        })]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .build(client, tools, store)
            .await;

        assert!(agent.comms().is_some());

        // Run agent - it should complete even with comms enabled and empty inbox
        let result = agent.run("Test".to_string()).await.unwrap();

        // Should reach final response (turn boundary was crossed successfully)
        assert_eq!(result.text, "Done after turn boundary");
        assert_eq!(result.turns, 2);
    }

    #[test]
    fn test_agent_formats_inbox_for_llm() {
        // Test that CommsMessage::to_user_message_text produces proper format.
        // This is already tested in comms_runtime::tests::test_comms_message_formatting
        // but we add a quick sanity check here for the integration context.
        use crate::comms_runtime::{CommsContent, CommsMessage};
        use meerkat_comms::PubKey;

        let msg = CommsMessage {
            id: uuid::Uuid::new_v4(),
            from_peer: "alice".to_string(),
            from_pubkey: PubKey::new([1u8; 32]),
            content: CommsContent::Message {
                body: "Hello from Alice".to_string(),
            },
        };

        let text = msg.to_user_message_text();

        // Should be formatted for LLM injection
        assert!(text.contains("[Comms]"), "Should have [Comms] prefix");
        assert!(text.contains("alice"), "Should include peer name");
        assert!(
            text.contains("Hello from Alice"),
            "Should include message body"
        );
    }

    // =========================================================================
    // Provider params tests (TDD)
    // =========================================================================

    #[test]
    fn test_agent_config_accepts_provider_params() {
        use crate::config::AgentConfig;

        // Test that AgentConfig has provider_params field
        let mut config = AgentConfig::default();
        assert!(config.provider_params.is_none());

        // Set provider params
        config.provider_params = Some(serde_json::json!({
            "thinking": {
                "type": "enabled",
                "budget_tokens": 10000
            }
        }));

        assert!(config.provider_params.is_some());
        let params = config.provider_params.unwrap();
        assert_eq!(params["thinking"]["budget_tokens"], 10000);
    }

    #[tokio::test]
    async fn test_agent_builder_provider_params() {
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Test builder has provider_params method
        let _agent = AgentBuilder::new()
            .model("test-model")
            .provider_params(serde_json::json!({
                "thinking": {
                    "type": "enabled",
                    "budget_tokens": 5000
                }
            }))
            .build(client, tools, store)
            .await;

        // Agent builds successfully with provider_params
    }

    /// Mock client that captures provider_params for verification
    struct ProviderParamsCapturingClient {
        captured_params: Mutex<Option<Value>>,
        responses: Mutex<Vec<LlmStreamResult>>,
    }

    impl ProviderParamsCapturingClient {
        fn new(responses: Vec<LlmStreamResult>) -> Self {
            Self {
                captured_params: Mutex::new(None),
                responses: Mutex::new(responses),
            }
        }

        fn captured_params(&self) -> Option<Value> {
            self.captured_params.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentLlmClient for ProviderParamsCapturingClient {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            provider_params: Option<&Value>,
        ) -> Result<LlmStreamResult, AgentError> {
            // Capture the provider_params
            *self.captured_params.lock().unwrap() = provider_params.cloned();

            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                Ok(LlmStreamResult::new(
                    "Default response".to_string(),
                    vec![],
                    StopReason::EndTurn,
                    Usage::default(),
                ))
            } else {
                Ok(responses.remove(0))
            }
        }

        fn provider(&self) -> &'static str {
            "mock-capturing"
        }
    }

    #[tokio::test]
    async fn test_provider_params_flows_to_stream_response() {
        let client = Arc::new(ProviderParamsCapturingClient::new(vec![LlmStreamResult {
            content: "Response with thinking".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let thinking_config = serde_json::json!({
            "thinking": {
                "type": "enabled",
                "budget_tokens": 10000
            }
        });

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .provider_params(thinking_config.clone())
            .build(client.clone(), tools, store)
            .await;

        let _result = agent.run("Test".to_string()).await.unwrap();

        // Verify provider_params was passed to stream_response
        let captured = client.captured_params();
        assert!(captured.is_some(), "provider_params should be captured");
        assert_eq!(captured.unwrap(), thinking_config);
    }

    #[tokio::test]
    async fn test_provider_params_none_when_not_set() {
        let client = Arc::new(ProviderParamsCapturingClient::new(vec![LlmStreamResult {
            content: "Response".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        }]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Build without provider_params
        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client.clone(), tools, store)
            .await;

        let _result = agent.run("Test".to_string()).await.unwrap();

        // Verify provider_params was None
        let captured = client.captured_params();
        assert!(
            captured.is_none(),
            "provider_params should be None when not set"
        );
    }

    // =========================================================================
    // Performance fix tests (TDD)
    // =========================================================================

    /// Test that FilteredToolDispatcher uses HashSet for O(1) lookups
    #[test]
    fn test_filtered_tool_dispatcher_uses_hashset() {
        // FilteredToolDispatcher now uses HashSet internally for O(1) lookups.
        // This test verifies the behavioral correctness of the filtering.

        // Create a dispatcher with many tools
        let mut tools = Vec::new();
        for i in 0..100 {
            tools.push(Arc::new(ToolDef {
                name: format!("tool_{}", i),
                description: format!("Tool {}", i),
                input_schema: empty_object_schema(),
            }));
        }
        let inner = Arc::new(MockToolDispatcher::new(tools));

        // Allow only 50 tools
        let allowed: Vec<String> = (0..50).map(|i| format!("tool_{}", i)).collect();
        let filtered = FilteredToolDispatcher::new(inner, allowed);

        // Verify it filters correctly
        let filtered_tools = filtered.tools();
        assert_eq!(filtered_tools.len(), 50);

        // Check first and last allowed tool are present
        assert!(filtered_tools.iter().any(|t| t.name == "tool_0"));
        assert!(filtered_tools.iter().any(|t| t.name == "tool_49"));

        // Check disallowed tool is not present
        assert!(!filtered_tools.iter().any(|t| t.name == "tool_50"));
    }

    #[tokio::test]
    async fn test_filtered_tool_dispatcher_blocks_disallowed() {
        let tools = vec![
            Arc::new(ToolDef {
                name: "allowed_tool".to_string(),
                description: "Allowed".to_string(),
                input_schema: empty_object_schema(),
            }),
            Arc::new(ToolDef {
                name: "blocked_tool".to_string(),
                description: "Blocked".to_string(),
                input_schema: empty_object_schema(),
            }),
        ];
        let inner = Arc::new(MockToolDispatcher::new(tools));
        let allowed = vec!["allowed_tool".to_string()];
        let filtered = FilteredToolDispatcher::new(inner, allowed);

        // Allowed tool should work
        let result = filtered
            .dispatch("allowed_tool", &serde_json::json!({}))
            .await;
        assert!(result.is_ok());

        // Blocked tool should fail
        let result = filtered
            .dispatch("blocked_tool", &serde_json::json!({}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not allowed"));
    }

    /// Test that tool_results Vec is pre-allocated
    #[tokio::test]
    async fn test_tool_results_vec_preallocated() {
        // This is a behavioral test - we verify multiple tool calls work correctly.
        // The pre-allocation is an implementation detail verified via code review.
        let client = Arc::new(MockLlmClient::new(vec![
            LlmStreamResult {
                content: "Calling tools".to_string(),
                tool_calls: vec![
                    ToolCall::new(
                        "tc_1".to_string(),
                        "tool_a".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_2".to_string(),
                        "tool_b".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_3".to_string(),
                        "tool_c".to_string(),
                        serde_json::json!({}),
                    ),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            LlmStreamResult {
                content: "Done".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));

        let tools = Arc::new(MockToolDispatcher::new(vec![
            Arc::new(ToolDef {
                name: "tool_a".to_string(),
                description: "A".to_string(),
                input_schema: empty_object_schema(),
            }),
            Arc::new(ToolDef {
                name: "tool_b".to_string(),
                description: "B".to_string(),
                input_schema: empty_object_schema(),
            }),
            Arc::new(ToolDef {
                name: "tool_c".to_string(),
                description: "C".to_string(),
                input_schema: empty_object_schema(),
            }),
        ]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools, store)
            .await;

        let result = agent.run("Test".to_string()).await.unwrap();

        assert_eq!(result.tool_calls, 3);
        assert_eq!(result.text, "Done");
    }

    /// Mock tool dispatcher that tracks execution order and timing for parallel execution tests
    struct TimingToolDispatcher {
        tools: Vec<ToolDef>,
        /// Records (tool_name, start_time, end_time) for each dispatch
        timings: std::sync::Arc<Mutex<Vec<(String, std::time::Instant, std::time::Instant)>>>,
        /// How long each tool should "execute" (simulated delay)
        delay_ms: u64,
    }

    impl TimingToolDispatcher {
        fn new(tools: Vec<ToolDef>, delay_ms: u64) -> Self {
            Self {
                tools,
                timings: std::sync::Arc::new(Mutex::new(Vec::new())),
                delay_ms,
            }
        }

        fn timings(&self) -> Vec<(String, std::time::Instant, std::time::Instant)> {
            self.timings.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for TimingToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            let tools: Vec<Arc<ToolDef>> = self.tools.iter().cloned().map(Arc::new).collect();
            Arc::from(tools)
        }

        async fn dispatch(
            &self,
            name: &str,
            _args: &Value,
        ) -> Result<Value, crate::error::ToolError> {
            let start = std::time::Instant::now();
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            let end = std::time::Instant::now();

            self.timings
                .lock()
                .unwrap()
                .push((name.to_string(), start, end));

            Ok(Value::String(format!("Result from {}", name)))
        }
    }

    /// Test that tool calls execute in parallel, not serially
    #[tokio::test]
    async fn test_tool_calls_execute_in_parallel() {
        let tool_delay_ms = 50;
        let num_tools = 3;

        let client = Arc::new(MockLlmClient::new(vec![
            LlmStreamResult {
                content: "Calling tools".to_string(),
                tool_calls: vec![
                    ToolCall::new(
                        "tc_1".to_string(),
                        "slow_tool_1".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_2".to_string(),
                        "slow_tool_2".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_3".to_string(),
                        "slow_tool_3".to_string(),
                        serde_json::json!({}),
                    ),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            LlmStreamResult {
                content: "Done".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));

        let tools = Arc::new(TimingToolDispatcher::new(
            vec![
                ToolDef {
                    name: "slow_tool_1".to_string(),
                    description: "Slow 1".to_string(),
                    input_schema: empty_object_schema(),
                },
                ToolDef {
                    name: "slow_tool_2".to_string(),
                    description: "Slow 2".to_string(),
                    input_schema: empty_object_schema(),
                },
                ToolDef {
                    name: "slow_tool_3".to_string(),
                    description: "Slow 3".to_string(),
                    input_schema: empty_object_schema(),
                },
            ],
            tool_delay_ms,
        ));
        let store = Arc::new(MockSessionStore);

        let overall_start = std::time::Instant::now();
        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools.clone(), store)
            .await;

        let result = agent.run("Test".to_string()).await.unwrap();
        let overall_duration = overall_start.elapsed();

        assert_eq!(result.tool_calls, 3);

        // If tools ran in parallel, total time should be ~tool_delay_ms (plus overhead)
        // If they ran serially, total time would be ~num_tools * tool_delay_ms
        let serial_time_ms = (num_tools * tool_delay_ms) as u128;

        assert!(
            overall_duration.as_millis() < serial_time_ms,
            "Tool execution took {}ms, which suggests serial execution (expected < {}ms for parallel)",
            overall_duration.as_millis(),
            serial_time_ms
        );

        // Verify execution times overlap (parallel execution)
        let timings = tools.timings();
        assert_eq!(timings.len(), 3, "Should have 3 timing records");

        // Check that tools started within a small window of each other (parallel start)
        let starts: Vec<_> = timings.iter().map(|(_, s, _)| *s).collect();
        let first_start = *starts.iter().min().unwrap();
        let last_start = *starts.iter().max().unwrap();
        let start_spread = last_start.duration_since(first_start);

        // If parallel, all tools should start within ~10ms of each other
        assert!(
            start_spread.as_millis() < 20,
            "Tools started {}ms apart, suggesting serial execution",
            start_spread.as_millis()
        );
    }

    /// Tool dispatcher that can simulate failures for specific tools
    struct FailingToolDispatcher {
        tools: Vec<ToolDef>,
        /// Tools that should fail (by name)
        failing_tools: std::collections::HashSet<String>,
        /// Delay in ms for each tool
        delay_ms: u64,
        /// Records dispatch order
        dispatch_order: std::sync::Arc<Mutex<Vec<String>>>,
    }

    impl FailingToolDispatcher {
        fn new(tools: Vec<ToolDef>, failing_tools: Vec<&str>, delay_ms: u64) -> Self {
            Self {
                tools,
                failing_tools: failing_tools.into_iter().map(|s| s.to_string()).collect(),
                delay_ms,
                dispatch_order: std::sync::Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn dispatch_order(&self) -> Vec<String> {
            self.dispatch_order.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for FailingToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            let tools: Vec<Arc<ToolDef>> = self.tools.iter().cloned().map(Arc::new).collect();
            Arc::from(tools)
        }

        async fn dispatch(
            &self,
            name: &str,
            _args: &Value,
        ) -> Result<Value, crate::error::ToolError> {
            // Record that dispatch started
            self.dispatch_order.lock().unwrap().push(name.to_string());

            // Simulate work
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;

            if self.failing_tools.contains(name) {
                Err(crate::error::ToolError::execution_failed(format!(
                    "Tool {} failed intentionally",
                    name
                )))
            } else {
                Ok(Value::String(format!("Success from {}", name)))
            }
        }
    }

    /// Test that parallel tool results preserve order (results match call order)
    #[tokio::test]
    async fn test_parallel_tool_results_preserve_order() {
        let client = Arc::new(MockLlmClient::new(vec![
            LlmStreamResult {
                content: "Calling tools".to_string(),
                tool_calls: vec![
                    ToolCall::new(
                        "tc_1".to_string(),
                        "tool_a".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_2".to_string(),
                        "tool_b".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_3".to_string(),
                        "tool_c".to_string(),
                        serde_json::json!({}),
                    ),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            LlmStreamResult {
                content: "Done".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));

        let tools = Arc::new(FailingToolDispatcher::new(
            vec![
                ToolDef {
                    name: "tool_a".to_string(),
                    description: "Tool A".to_string(),
                    input_schema: empty_object_schema(),
                },
                ToolDef {
                    name: "tool_b".to_string(),
                    description: "Tool B".to_string(),
                    input_schema: empty_object_schema(),
                },
                ToolDef {
                    name: "tool_c".to_string(),
                    description: "Tool C".to_string(),
                    input_schema: empty_object_schema(),
                },
            ],
            vec![], // No failures
            10,     // 10ms delay
        ));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools.clone(), store)
            .await;

        let _result = agent.run("Test".to_string()).await.unwrap();

        // Check session has tool results in correct order
        let messages = agent.session().messages();
        let tool_results_msg = messages
            .iter()
            .find(|m| matches!(m, Message::ToolResults { .. }));
        assert!(
            tool_results_msg.is_some(),
            "Should have tool results message"
        );

        if let Message::ToolResults { results } = tool_results_msg.unwrap() {
            assert_eq!(results.len(), 3);
            assert_eq!(
                results[0].tool_use_id, "tc_1",
                "First result should be tc_1"
            );
            assert_eq!(
                results[1].tool_use_id, "tc_2",
                "Second result should be tc_2"
            );
            assert_eq!(
                results[2].tool_use_id, "tc_3",
                "Third result should be tc_3"
            );
        }
    }

    /// Test that partial failures don't block other tools
    #[tokio::test]
    async fn test_parallel_tool_partial_failure() {
        let client = Arc::new(MockLlmClient::new(vec![
            LlmStreamResult {
                content: "Calling tools".to_string(),
                tool_calls: vec![
                    ToolCall::new(
                        "tc_1".to_string(),
                        "good_tool".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_2".to_string(),
                        "bad_tool".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_3".to_string(),
                        "another_good_tool".to_string(),
                        serde_json::json!({}),
                    ),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            LlmStreamResult {
                content: "Done".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));

        let tools = Arc::new(FailingToolDispatcher::new(
            vec![
                ToolDef {
                    name: "good_tool".to_string(),
                    description: "Good Tool".to_string(),
                    input_schema: empty_object_schema(),
                },
                ToolDef {
                    name: "bad_tool".to_string(),
                    description: "Bad Tool".to_string(),
                    input_schema: empty_object_schema(),
                },
                ToolDef {
                    name: "another_good_tool".to_string(),
                    description: "Another Good Tool".to_string(),
                    input_schema: empty_object_schema(),
                },
            ],
            vec!["bad_tool"], // Only bad_tool fails
            10,
        ));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools.clone(), store)
            .await;

        // Should complete successfully even with partial failure
        let result = agent.run("Test".to_string()).await;
        assert!(
            result.is_ok(),
            "Agent should complete even with partial tool failure"
        );

        // Check that all tools were called
        let dispatch_order = tools.dispatch_order();
        assert_eq!(
            dispatch_order.len(),
            3,
            "All 3 tools should have been dispatched"
        );

        // Check session has correct results with error flags
        let messages = agent.session().messages();
        let tool_results_msg = messages
            .iter()
            .find(|m| matches!(m, Message::ToolResults { .. }));

        if let Some(Message::ToolResults { results }) = tool_results_msg {
            assert_eq!(results.len(), 3);

            // First tool succeeded
            assert!(!results[0].is_error, "good_tool should succeed");
            assert!(results[0].content.contains("Success"));

            // Second tool failed
            assert!(results[1].is_error, "bad_tool should fail");
            assert!(results[1].content.contains("failed"));

            // Third tool succeeded
            assert!(!results[2].is_error, "another_good_tool should succeed");
            assert!(results[2].content.contains("Success"));
        }
    }

    /// Test that all tools failing still completes the turn
    #[tokio::test]
    async fn test_parallel_tool_all_failures() {
        let client = Arc::new(MockLlmClient::new(vec![
            LlmStreamResult {
                content: "Calling tools".to_string(),
                tool_calls: vec![
                    ToolCall::new(
                        "tc_1".to_string(),
                        "fail1".to_string(),
                        serde_json::json!({}),
                    ),
                    ToolCall::new(
                        "tc_2".to_string(),
                        "fail2".to_string(),
                        serde_json::json!({}),
                    ),
                ],
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
            },
            LlmStreamResult {
                content: "I see both tools failed".to_string(),
                tool_calls: vec![],
                stop_reason: StopReason::EndTurn,
                usage: Usage::default(),
            },
        ]));

        let tools = Arc::new(FailingToolDispatcher::new(
            vec![
                ToolDef {
                    name: "fail1".to_string(),
                    description: "Fail 1".to_string(),
                    input_schema: empty_object_schema(),
                },
                ToolDef {
                    name: "fail2".to_string(),
                    description: "Fail 2".to_string(),
                    input_schema: empty_object_schema(),
                },
            ],
            vec!["fail1", "fail2"], // Both fail
            10,
        ));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new()
            .model("test-model")
            .build(client, tools.clone(), store)
            .await;

        // Should complete - failures go back to LLM as error results
        let result = agent.run("Test".to_string()).await;
        assert!(
            result.is_ok(),
            "Agent should complete even when all tools fail"
        );

        // Check both errors were captured
        let messages = agent.session().messages();
        let tool_results_msg = messages
            .iter()
            .find(|m| matches!(m, Message::ToolResults { .. }));

        if let Some(Message::ToolResults { results }) = tool_results_msg {
            assert_eq!(results.len(), 2);
            assert!(results[0].is_error);
            assert!(results[1].is_error);
        }
    }

    // =========================================================================
    // Host Mode Tests
    // =========================================================================

    #[tokio::test]
    async fn test_run_host_mode_requires_comms() {
        // Host mode should fail if comms is not enabled
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        let mut agent = AgentBuilder::new().build(client, tools, store).await;

        // Verify comms is not enabled
        assert!(agent.comms().is_none());

        // run_host_mode should return ConfigError
        let result = agent.run_host_mode("Test".to_string()).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            AgentError::ConfigError(msg) => {
                assert!(msg.contains("comms"));
            }
            other => unreachable!("Expected ConfigError, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_run_host_mode_runs_initial_prompt() {
        use crate::comms_config::CoreCommsConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        config.listen_uds = None;
        config.listen_tcp = None;

        // Create client that responds then exhausts budget
        let client = Arc::new(MockLlmClient::new(vec![LlmStreamResult {
            content: "Initial response".to_string(),
            tool_calls: vec![],
            stop_reason: StopReason::EndTurn,
            usage: Usage {
                input_tokens: 100,
                output_tokens: 100,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        }]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Set a very small budget so we exit after initial prompt
        let mut agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .budget(BudgetLimits {
                max_tokens: Some(1), // Exhaust after first run
                max_duration: None,
                max_tool_calls: None,
            })
            .build(client, tools, store)
            .await;

        // Verify comms is enabled
        assert!(agent.comms().is_some());

        // run_host_mode should run the initial prompt then exit due to budget
        let result = agent.run_host_mode("Hello".to_string()).await;

        // Should succeed with the initial response
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.text, "Initial response");
    }

    #[tokio::test]
    async fn test_run_host_mode_empty_prompt_skips_llm() {
        use crate::comms_config::CoreCommsConfig;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        config.listen_uds = None;
        config.listen_tcp = None;

        // Create client that tracks call count (via responses consumed)
        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Set very small budget - if LLM is called, it will be exhausted
        let mut agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .budget(BudgetLimits {
                max_tokens: Some(1), // Would be exhausted if LLM called
                max_duration: Some(std::time::Duration::from_millis(50)),
                max_tool_calls: None,
            })
            .build(client, tools, store)
            .await;

        // Empty prompt - should skip LLM, wait briefly, then exit on duration timeout
        let result = agent.run_host_mode("".to_string()).await;

        // Should succeed with empty result (no LLM call made)
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(
            result.text.is_empty(),
            "Empty prompt should result in empty text"
        );
        assert_eq!(result.turns, 0, "No turns should have been run");
    }

    #[tokio::test]
    async fn test_run_host_mode_duration_timeout() {
        use crate::comms_config::CoreCommsConfig;
        use std::time::{Duration, Instant};
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let mut config = CoreCommsConfig::with_name("test-agent");
        config.identity_dir = tmp.path().join("identity");
        config.trusted_peers_path = tmp.path().join("trusted.json");
        config.listen_uds = None;
        config.listen_tcp = None;

        let client = Arc::new(MockLlmClient::new(vec![]));
        let tools = Arc::new(MockToolDispatcher::new(vec![]));
        let store = Arc::new(MockSessionStore);

        // Set a short duration limit
        let mut agent = AgentBuilder::new()
            .comms(config)
            .comms_base_dir(tmp.path().to_path_buf())
            .budget(BudgetLimits {
                max_tokens: None,
                max_duration: Some(Duration::from_millis(100)),
                max_tool_calls: None,
            })
            .build(client, tools, store)
            .await;

        let start = Instant::now();
        let result = agent.run_host_mode("".to_string()).await;
        let elapsed = start.elapsed();

        // Should exit due to duration timeout
        assert!(result.is_ok());
        // Should have waited at least close to the duration
        assert!(
            elapsed >= Duration::from_millis(50),
            "Should have waited for timeout, but only waited {:?}",
            elapsed
        );
        // Should not have waited too long (well over the 100ms limit)
        assert!(
            elapsed < Duration::from_secs(2),
            "Should have exited on timeout, but waited {:?}",
            elapsed
        );
    }
}
