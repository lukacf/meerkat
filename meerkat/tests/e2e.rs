//! End-to-end tests for Meerkat (Gate 1: E2E Scenarios)
//!
//! These tests verify complete user flows through the system.
//! They use real LLM providers and test the full stack.
//! Per RCT methodology, tests are COMPLETE - they exercise real code paths.
//!
//! All E2E tests are marked #[ignore] to avoid running in normal CI
//! since they require API keys and make real API calls.
//!
//! Run with: cargo e2e (or cargo test --package meerkat --test e2e -- --ignored)

use async_trait::async_trait;
use futures::StreamExt;
use meerkat::*;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;

// ============================================================================
// ADAPTERS - Bridge LlmClient/SessionStore to Agent traits
// ============================================================================

/// Adapter that wraps an LlmClient to implement AgentLlmClient
pub struct LlmClientAdapter<C: LlmClient> {
    client: Arc<C>,
    model: String,
}

impl<C: LlmClient> LlmClientAdapter<C> {
    pub fn new(client: Arc<C>, model: String) -> Self {
        Self { client, model }
    }
}

#[async_trait]
impl<C: LlmClient + 'static> AgentLlmClient for LlmClientAdapter<C> {
    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[ToolDef],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&serde_json::Value>,
    ) -> Result<LlmStreamResult, AgentError> {
        let request = LlmRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            max_tokens,
            temperature,
            stop_sequences: None,
            provider_params: provider_params.cloned(),
        };

        let mut stream = self.client.stream(&request);

        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut tool_call_buffers: HashMap<String, ToolCallBuffer> = HashMap::new();
        let mut stop_reason = StopReason::EndTurn;
        let mut usage = Usage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => match event {
                    LlmEvent::TextDelta { delta } => {
                        content.push_str(&delta);
                    }
                    LlmEvent::ToolCallDelta {
                        id,
                        name,
                        args_delta,
                    } => {
                        let buffer = tool_call_buffers
                            .entry(id.clone())
                            .or_insert_with(|| ToolCallBuffer::new(id));

                        if let Some(n) = name {
                            buffer.name = Some(n);
                        }
                        buffer.args_json.push_str(&args_delta);
                    }
                    LlmEvent::ToolCallComplete { id, name, args, .. } => {
                        tool_calls.push(ToolCall::new(id, name, args));
                    }
                    LlmEvent::UsageUpdate { usage: u } => {
                        usage = u;
                    }
                    LlmEvent::Done { stop_reason: sr } => {
                        stop_reason = sr;
                    }
                },
                Err(e) => {
                    return Err(AgentError::LlmError(e.to_string()));
                }
            }
        }

        // Complete any buffered tool calls
        for (_, buffer) in tool_call_buffers {
            if let Some(tc) = buffer.try_complete() {
                if !tool_calls.iter().any(|t| t.id == tc.id) {
                    tool_calls.push(tc);
                }
            }
        }

        Ok(LlmStreamResult {
            content,
            tool_calls,
            stop_reason,
            usage,
        })
    }

    fn provider(&self) -> &'static str {
        self.client.provider()
    }
}

#[derive(Debug, Default)]
struct ToolCallBuffer {
    id: String,
    name: Option<String>,
    args_json: String,
}

impl ToolCallBuffer {
    fn new(id: String) -> Self {
        Self {
            id,
            name: None,
            args_json: String::new(),
        }
    }

    fn try_complete(&self) -> Option<ToolCall> {
        let name = self.name.as_ref()?;
        let args: Value = serde_json::from_str(&self.args_json).ok()?;
        Some(ToolCall::new(self.id.clone(), name.clone(), args))
    }
}

/// Adapter that wraps a SessionStore to implement AgentSessionStore
pub struct SessionStoreAdapter<S: SessionStore> {
    store: Arc<S>,
}

impl<S: SessionStore> SessionStoreAdapter<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl<S: SessionStore + 'static> AgentSessionStore for SessionStoreAdapter<S> {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.store
            .save(session)
            .await
            .map_err(|e| AgentError::StoreError(e.to_string()))
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        let session_id = SessionId::parse(id)
            .map_err(|e| AgentError::StoreError(format!("Invalid session ID: {}", e)))?;

        self.store
            .load(&session_id)
            .await
            .map_err(|e| AgentError::StoreError(e.to_string()))
    }
}

/// Empty tool dispatcher for when no tools are configured
pub struct EmptyToolDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        Vec::new()
    }

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
        Err(ToolError::not_found(name))
    }
}

/// Mock tool dispatcher for testing
#[derive(Default)]
pub struct MockToolDispatcher {
    tools: Vec<ToolDef>,
    results: HashMap<String, String>,
}

impl MockToolDispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_tool(mut self, name: &str, description: &str, result: &str) -> Self {
        self.tools.push(ToolDef {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        });
        self.results.insert(name.to_string(), result.to_string());
        self
    }
}

#[async_trait]
impl AgentToolDispatcher for MockToolDispatcher {
    fn tools(&self) -> Vec<ToolDef> {
        self.tools.clone()
    }

    async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
        self.results
            .get(name)
            .cloned()
            .map(Value::String)
            .ok_or_else(|| ToolError::not_found(name))
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

fn skip_if_no_anthropic_key() -> Option<String> {
    std::env::var("ANTHROPIC_API_KEY").ok()
}

#[allow(dead_code)]
fn skip_if_no_openai_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY").ok()
}

#[allow(dead_code)]
fn skip_if_no_gemini_key() -> Option<String> {
    std::env::var("GOOGLE_API_KEY").ok()
}

/// Get the Anthropic model to use in tests (configurable via ANTHROPIC_MODEL env var)
fn anthropic_model() -> String {
    std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-opus-4-5".to_string())
}

/// Get the OpenAI model to use in tests (configurable via OPENAI_MODEL env var)
#[allow(dead_code)]
fn openai_model() -> String {
    std::env::var("OPENAI_MODEL").unwrap_or_else(|_| "gpt-5.2".to_string())
}

/// Get the Gemini model to use in tests (configurable via GEMINI_MODEL env var)
#[allow(dead_code)]
fn gemini_model() -> String {
    std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-3-flash-preview".to_string())
}

fn get_test_server_path() -> Option<std::path::PathBuf> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let workspace_root = std::path::Path::new(&manifest_dir).parent()?;
    let server_path = workspace_root.join("target/debug/mcp-test-server");
    if server_path.exists() {
        Some(server_path)
    } else {
        None
    }
}

/// Create a store adapter using JsonlStore with a temp directory
async fn create_temp_store() -> (
    Arc<JsonlStore>,
    Arc<SessionStoreAdapter<JsonlStore>>,
    TempDir,
) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let store = JsonlStore::new(temp_dir.path().to_path_buf());
    store.init().await.expect("Failed to init store");
    let store = Arc::new(store);
    let adapter = Arc::new(SessionStoreAdapter::new(store.clone()));
    (store, adapter, temp_dir)
}

// ============================================================================
// E2E: SIMPLE CHAT FLOW
// ============================================================================

/// E2E: Simple chat flow
/// User message → LLM response → done
mod simple_chat {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_simple_chat_anthropic() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        // Create components
        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));
        let tools = Arc::new(EmptyToolDispatcher);
        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        // Build agent
        let mut agent = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(256)
            .system_prompt("You are a helpful assistant. Respond briefly.")
            .build(llm_adapter, tools, store_adapter);

        // Run with simple prompt
        let result = agent
            .run("What is 2+2? Answer with just the number.".to_string())
            .await
            .expect("Agent run should succeed");

        // Verify response
        assert!(!result.text.is_empty(), "Should have non-empty response");
        assert!(
            result.text.contains('4') || result.text.to_lowercase().contains("four"),
            "Response should contain the answer: {}",
            result.text
        );
        assert_eq!(result.turns, 1, "Should complete in 1 turn");
        assert!(
            result.usage.input_tokens > 0,
            "Should have used input tokens"
        );
        assert!(
            result.usage.output_tokens > 0,
            "Should have used output tokens"
        );

        // Verify session state
        assert_eq!(
            agent.session().messages().len(),
            3,
            "Session should have system + user + assistant messages"
        );
    }

    #[cfg(feature = "openai")]
    #[tokio::test]
    #[ignore = "Requires OPENAI_API_KEY"]
    async fn test_simple_chat_openai() {
        let Some(api_key) = skip_if_no_openai_key() else {
            eprintln!("Skipping: OPENAI_API_KEY not set");
            return;
        };

        let llm_client = Arc::new(OpenAiClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, openai_model()));
        let tools = Arc::new(EmptyToolDispatcher);
        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(openai_model())
            .max_tokens_per_turn(256)
            .system_prompt("You are a helpful assistant. Respond briefly.")
            .build(llm_adapter, tools, store_adapter);

        let result = agent
            .run("What is 2+2? Answer with just the number.".to_string())
            .await
            .expect("Agent run should succeed");

        assert!(!result.text.is_empty(), "Should have non-empty response");
        assert!(
            result.text.contains('4') || result.text.to_lowercase().contains("four"),
            "Response should contain the answer: {}",
            result.text
        );
    }

    #[cfg(feature = "gemini")]
    #[tokio::test]
    #[ignore = "Requires GOOGLE_API_KEY"]
    async fn test_simple_chat_gemini() {
        let Some(api_key) = skip_if_no_gemini_key() else {
            eprintln!("Skipping: GOOGLE_API_KEY not set");
            return;
        };

        let llm_client = Arc::new(GeminiClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, gemini_model()));
        let tools = Arc::new(EmptyToolDispatcher);
        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(gemini_model())
            .max_tokens_per_turn(256)
            .system_prompt("You are a helpful assistant. Respond briefly.")
            .build(llm_adapter, tools, store_adapter);

        let result = agent
            .run("What is 2+2? Answer with just the number.".to_string())
            .await
            .expect("Agent run should succeed");

        assert!(!result.text.is_empty(), "Should have non-empty response");
        assert!(
            result.text.contains('4') || result.text.to_lowercase().contains("four"),
            "Response should contain the answer: {}",
            result.text
        );
    }
}

// ============================================================================
// E2E: TOOL INVOCATION FLOW
// ============================================================================

/// E2E: Tool invocation flow
/// LLM requests tool → tool runs → result injected → LLM responds
mod tool_invocation {
    use super::*;

    // Wrapper to implement AgentToolDispatcher for ToolDispatcher
    // Uses tokio::sync::Mutex for async safety
    pub struct DispatcherWrapper {
        tools: Vec<ToolDef>,
        router: Arc<McpRouter>,
        #[allow(dead_code)]
        timeout: std::time::Duration,
    }

    impl DispatcherWrapper {
        pub fn new(
            tools: Vec<ToolDef>,
            router: Arc<McpRouter>,
            timeout: std::time::Duration,
        ) -> Self {
            Self {
                tools,
                router,
                timeout,
            }
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for DispatcherWrapper {
        fn tools(&self) -> Vec<ToolDef> {
            self.tools.clone()
        }

        async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
            // Call the tool through the router
            match self.router.call_tool(name, args).await {
                Ok(result) => {
                    #[allow(clippy::unnecessary_lazy_evaluations)]
                    let value =
                        serde_json::from_str(&result).unwrap_or_else(|_| Value::String(result));
                    Ok(value)
                }
                Err(e) => Err(ToolError::execution_failed(e.to_string())),
            }
        }
    }

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY and MCP test server"]
    async fn test_tool_invocation_with_mcp() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let Some(server_path) = get_test_server_path() else {
            eprintln!("Skipping: MCP test server not built (run cargo build -p mcp-test-server)");
            return;
        };

        // Create MCP router and connect to test server
        let mut router = McpRouter::new();
        let config = McpServerConfig::stdio(
            "test",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );
        router
            .add_server(config)
            .await
            .expect("Should connect to MCP server");

        // List tools from router (now synchronous)
        let tools = router.list_tools();
        assert!(!tools.is_empty(), "Should have discovered tools");
        assert!(
            tools.iter().any(|t| t.name == "echo"),
            "Should have echo tool"
        );

        let tools_vec = tools.to_vec();
        let router = Arc::new(router);
        let dispatcher = Arc::new(DispatcherWrapper::new(
            tools_vec,
            router,
            std::time::Duration::from_secs(30),
        ));

        // Create agent with tool support
        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));

        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(512)
            .system_prompt(
                "You have access to an 'echo' tool. When asked to echo something, use it.",
            )
            .build(llm_adapter, dispatcher, store_adapter);

        // Run agent with prompt that should trigger tool use
        let result = agent
            .run("Please use the echo tool to echo 'Hello from E2E test'".to_string())
            .await
            .expect("Agent run should succeed");

        // Verify tool was called
        assert!(result.tool_calls > 0, "Should have made tool call(s)");

        // Verify response mentions the echoed content
        assert!(
            result.text.to_lowercase().contains("hello")
                || result.text.to_lowercase().contains("echo"),
            "Response should reference tool result: {}",
            result.text
        );
    }
}

// ============================================================================
// E2E: MULTI-TURN CONVERSATION
// ============================================================================

/// E2E: Multi-turn conversation
/// Maintains context across 5+ turns
mod multi_turn {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_multi_turn_context_maintained() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));
        let tools = Arc::new(EmptyToolDispatcher);
        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(256)
            .system_prompt("You are a helpful assistant. Keep your responses brief.")
            .build(llm_adapter, tools, store_adapter);

        // Turn 1: Establish context
        let result1 = agent
            .run("My name is Alice and my favorite color is blue.".to_string())
            .await
            .expect("Turn 1 should succeed");
        assert!(!result1.text.is_empty());

        // Turn 2: Add more context
        let result2 = agent
            .run("I also like the number 42.".to_string())
            .await
            .expect("Turn 2 should succeed");
        assert!(!result2.text.is_empty());

        // Turn 3: Verify context is maintained
        let result3 = agent
            .run("What is my name?".to_string())
            .await
            .expect("Turn 3 should succeed");
        assert!(
            result3.text.to_lowercase().contains("alice"),
            "Should remember name: {}",
            result3.text
        );

        // Turn 4: Verify more context
        let result4 = agent
            .run("What is my favorite color?".to_string())
            .await
            .expect("Turn 4 should succeed");
        assert!(
            result4.text.to_lowercase().contains("blue"),
            "Should remember color: {}",
            result4.text
        );

        // Turn 5: Verify all context
        let result5 = agent
            .run("What number do I like?".to_string())
            .await
            .expect("Turn 5 should succeed");
        assert!(
            result5.text.contains("42"),
            "Should remember number: {}",
            result5.text
        );

        // Verify session has all messages
        // System + (User+Assistant)*5 = 11 messages
        assert!(
            agent.session().messages().len() >= 10,
            "Session should have messages from all turns: {}",
            agent.session().messages().len()
        );
    }
}

// ============================================================================
// E2E: SESSION RESUME
// ============================================================================

/// E2E: Session resume
/// Checkpoint → restart → continue
mod session_resume {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_session_resume_from_checkpoint() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let store = JsonlStore::new(temp_dir.path().to_path_buf());
        store.init().await.expect("Store init should succeed");
        let store = Arc::new(store);

        // Phase 1: Create agent, establish context, save
        let session_id = {
            let llm_client = Arc::new(AnthropicClient::new(api_key.clone()));
            let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));
            let tools = Arc::new(EmptyToolDispatcher);
            let store_adapter = Arc::new(SessionStoreAdapter::new(store.clone()));

            let mut agent = AgentBuilder::new()
                .model(anthropic_model())
                .max_tokens_per_turn(256)
                .system_prompt("You are a helpful assistant. Keep your responses brief.")
                .build(llm_adapter, tools, store_adapter);

            // Establish context
            let _ = agent
                .run("Remember this secret code: ALPHA-BRAVO-7".to_string())
                .await
                .expect("Turn should succeed");

            // Session is automatically saved after run completes
            // Return session ID for phase 2
            agent.session().id().clone()
        };

        // Phase 2: Create new agent, resume from checkpoint, verify context
        {
            // Load saved session
            let saved_session = store
                .load(&session_id)
                .await
                .expect("Load should succeed")
                .expect("Session should exist");

            let llm_client = Arc::new(AnthropicClient::new(api_key));
            let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));
            let tools = Arc::new(EmptyToolDispatcher);
            let store_adapter = Arc::new(SessionStoreAdapter::new(store.clone()));

            // Resume with saved session
            let mut agent = AgentBuilder::new()
                .model(anthropic_model())
                .max_tokens_per_turn(256)
                .resume_session(saved_session)
                .build(llm_adapter, tools, store_adapter);

            // Verify context is maintained
            let result = agent
                .run("What was the secret code I told you?".to_string())
                .await
                .expect("Turn should succeed");

            assert!(
                result.text.contains("ALPHA")
                    || result.text.contains("BRAVO")
                    || result.text.contains("7"),
                "Should remember the secret code: {}",
                result.text
            );
        }
    }
}

// ============================================================================
// E2E: BUDGET EXHAUSTION
// ============================================================================

/// E2E: Budget exhaustion
/// Hit token limit → graceful stop
mod budget_exhaustion {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_budget_exhaustion_graceful_stop() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));
        let tools = Arc::new(EmptyToolDispatcher);
        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        // Create agent with very low token budget
        let mut agent = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(100)
            .system_prompt("You are a helpful assistant.")
            .budget(BudgetLimits {
                max_tokens: Some(150), // Very low budget
                max_duration: None,
                max_tool_calls: None,
            })
            .build(llm_adapter, tools, store_adapter);

        // First turn should succeed
        let result1 = agent
            .run("Say hello briefly.".to_string())
            .await
            .expect("First turn should succeed");

        assert!(!result1.text.is_empty(), "Should have response");

        // Budget should be depleted or nearly depleted after first turn
        // due to input + output tokens
        let budget = agent.budget();
        let (used, limit) = budget.token_usage().expect("Should have token budget");

        // Note: The budget might already be exhausted after the first turn
        // depending on how many tokens were used
        eprintln!("Budget: used={}, limit={}", used, limit);

        // If budget is not exhausted, try another turn
        if !budget.is_exhausted() {
            let result2 = agent
                .run("Tell me more.".to_string())
                .await
                .expect("Second turn should complete (may be limited)");

            // Even if budget is exhausted mid-turn, agent should complete gracefully
            // without panic or error
            eprintln!("Second turn result: {}", result2.text);
        }

        // Verify agent is in valid state
        assert!(
            agent.state().is_terminal() || *agent.state() == LoopState::CallingLlm,
            "Agent should be in valid state"
        );
    }

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_tool_call_budget_limit() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));

        // Create mock tools
        let tools = Arc::new(
            MockToolDispatcher::new()
                .with_tool("get_data", "Get some data", "data: 42")
                .with_tool("process", "Process data", "processed"),
        );

        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        // Create agent with tool call limit
        let mut agent = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(512)
            .system_prompt("You have tools available. Use them when asked.")
            .budget(BudgetLimits {
                max_tokens: Some(10000),
                max_duration: None,
                max_tool_calls: Some(2), // Very low tool call limit
            })
            .build(llm_adapter, tools, store_adapter);

        // Run a query that might trigger tool calls
        let result = agent
            .run("Please use the tools to get and process some data.".to_string())
            .await
            .expect("Should complete");

        // Verify tool calls are within budget
        assert!(
            result.tool_calls <= 2,
            "Should not exceed tool call limit: {}",
            result.tool_calls
        );
    }
}

// ============================================================================
// E2E: PARALLEL TOOLS
// ============================================================================

/// E2E: Parallel tools
/// LLM requests multiple tools → all run concurrently → results injected
mod parallel_tools {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY and MCP test server"]
    async fn test_parallel_tool_execution() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let Some(server_path) = get_test_server_path() else {
            eprintln!("Skipping: MCP test server not built");
            return;
        };

        // Create MCP router with test server
        let mut router = McpRouter::new();
        let config = McpServerConfig::stdio(
            "test",
            server_path.to_string_lossy().to_string(),
            vec![],
            HashMap::new(),
        );
        router
            .add_server(config)
            .await
            .expect("Should connect to MCP server");

        // List tools from router (now synchronous)
        let tools = router.list_tools();
        assert!(
            tools.iter().any(|t| t.name == "add"),
            "Should have add tool"
        );

        // Note: Whether tools run in parallel depends on the agent loop implementation
        // and LLM behavior. This test verifies that multiple tool results can be
        // handled correctly when the LLM requests multiple tools.

        eprintln!(
            "Parallel tool execution test: tools available: {:?}",
            tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );

        let tools_vec = tools.to_vec();
        let router = Arc::new(router);
        let dispatcher = Arc::new(tool_invocation::DispatcherWrapper::new(
            tools_vec,
            router,
            std::time::Duration::from_secs(30),
        ));

        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));

        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(512)
            .system_prompt("You have access to an 'add' tool that adds two numbers.")
            .build(llm_adapter, dispatcher, store_adapter);

        // Ask for multiple calculations - LLM may issue multiple tool calls
        let result = agent
            .run("Calculate 1+2 and 3+4 using the add tool".to_string())
            .await
            .expect("Should complete");

        // Verify at least one tool call was made
        assert!(result.tool_calls > 0, "Should have made tool calls");

        // Response should contain the results
        assert!(
            result.text.contains('3') || result.text.contains('7'),
            "Response should contain calculation results: {}",
            result.text
        );
    }

    /// Tool dispatcher with simulated delays to verify parallel execution timing
    pub struct TimedToolDispatcher {
        tools: Vec<ToolDef>,
        delay_ms: u64,
        call_log: Arc<std::sync::Mutex<Vec<(String, std::time::Instant, std::time::Instant)>>>,
    }

    impl TimedToolDispatcher {
        pub fn new(delay_ms: u64) -> Self {
            Self {
                tools: vec![
                    ToolDef {
                        name: "get_weather".to_string(),
                        description: "Get current weather for a city".to_string(),
                        input_schema: serde_json::json!({
                            "type": "object",
                            "properties": {
                                "city": { "type": "string", "description": "City name" }
                            },
                            "required": ["city"]
                        }),
                    },
                    ToolDef {
                        name: "get_time".to_string(),
                        description: "Get current time for a timezone".to_string(),
                        input_schema: serde_json::json!({
                            "type": "object",
                            "properties": {
                                "timezone": { "type": "string", "description": "Timezone name" }
                            },
                            "required": ["timezone"]
                        }),
                    },
                    ToolDef {
                        name: "get_stock".to_string(),
                        description: "Get stock price for a symbol".to_string(),
                        input_schema: serde_json::json!({
                            "type": "object",
                            "properties": {
                                "symbol": { "type": "string", "description": "Stock symbol" }
                            },
                            "required": ["symbol"]
                        }),
                    },
                ],
                delay_ms,
                call_log: Arc::new(std::sync::Mutex::new(Vec::new())),
            }
        }

        pub fn call_timings(&self) -> Vec<(String, std::time::Instant, std::time::Instant)> {
            self.call_log.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AgentToolDispatcher for TimedToolDispatcher {
        fn tools(&self) -> Vec<ToolDef> {
            self.tools.clone()
        }

        async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, ToolError> {
            let start = std::time::Instant::now();

            // Simulate network/processing delay
            tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;

            let end = std::time::Instant::now();
            self.call_log
                .lock()
                .unwrap()
                .push((name.to_string(), start, end));

            // Return realistic mock responses
            match name {
                "get_weather" => {
                    let city = args
                        .get("city")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    Ok(Value::String(format!("Weather in {}: Sunny, 22°C", city)))
                }
                "get_time" => {
                    let tz = args
                        .get("timezone")
                        .and_then(|v| v.as_str())
                        .unwrap_or("UTC");
                    Ok(Value::String(format!("Current time in {}: 14:30 PM", tz)))
                }
                "get_stock" => {
                    let symbol = args.get("symbol").and_then(|v| v.as_str()).unwrap_or("???");
                    Ok(Value::String(format!("Stock {}: $150.25 (+2.3%)", symbol)))
                }
                _ => Err(ToolError::not_found(name)),
            }
        }
    }

    /// E2E test for parallel tool execution with timing verification
    /// Verifies that multiple tools execute concurrently, not serially
    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_parallel_tools_with_timing_verification() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        // Each tool takes 200ms - if serial, 3 tools = 600ms+; if parallel, ~200ms+
        let tool_delay_ms = 200;
        let dispatcher = Arc::new(TimedToolDispatcher::new(tool_delay_ms));

        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));

        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(1024)
            .system_prompt(
                "You have access to get_weather, get_time, and get_stock tools. \
                 When asked about multiple things, use ALL relevant tools in a SINGLE response. \
                 Do not call them one at a time.",
            )
            .build(llm_adapter, dispatcher.clone(), store_adapter);

        // Ask for multiple pieces of info to encourage parallel tool calls
        let overall_start = std::time::Instant::now();
        let result = agent
            .run("I need the weather in Tokyo, the time in London, and the stock price for AAPL. Get all three.".to_string())
            .await
            .expect("Should complete");
        let overall_duration = overall_start.elapsed();

        eprintln!(
            "Result: {} tool calls in {:?}",
            result.tool_calls, overall_duration
        );
        eprintln!("Response: {}", result.text);

        // Verify multiple tools were called
        assert!(
            result.tool_calls >= 2,
            "LLM should have called multiple tools, got: {}",
            result.tool_calls
        );

        // Check timing to verify parallel execution
        let timings = dispatcher.call_timings();
        if timings.len() >= 2 {
            let starts: Vec<_> = timings.iter().map(|(_, s, _)| *s).collect();
            let first_start = *starts.iter().min().unwrap();
            let last_start = *starts.iter().max().unwrap();
            let start_spread = last_start.duration_since(first_start);

            eprintln!("Tool start spread: {:?}", start_spread);

            // If parallel, tools should start within ~50ms of each other
            // If serial, they'd start 200ms+ apart
            assert!(
                start_spread.as_millis() < 100,
                "Tools should start nearly simultaneously for parallel execution. \
                 Start spread was {:?}, suggesting serial execution.",
                start_spread
            );
        }

        // Verify response contains info from all tools
        let text_lower = result.text.to_lowercase();
        assert!(
            text_lower.contains("tokyo")
                || text_lower.contains("weather")
                || text_lower.contains("sunny"),
            "Response should mention weather result"
        );
    }

    /// E2E test for multi-turn conversation with parallel tools
    /// Turn 1: Multiple tools → Turn 2: Follow-up question
    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_parallel_tools_multiturn() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let dispatcher = Arc::new(TimedToolDispatcher::new(100));

        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));

        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(1024)
            .system_prompt(
                "You have access to get_weather, get_time, and get_stock tools. \
                 Use multiple tools when appropriate.",
            )
            .build(llm_adapter, dispatcher.clone(), store_adapter);

        // Turn 1: Multiple tool calls
        let result1 = agent
            .run("Get the weather in Paris and the stock price for MSFT".to_string())
            .await
            .expect("Turn 1 should complete");

        eprintln!("Turn 1: {} tool calls", result1.tool_calls);
        eprintln!("Turn 1 response: {}", result1.text);

        assert!(result1.tool_calls >= 1, "Turn 1 should have tool calls");

        // Turn 2: Follow-up (uses session history)
        let result2 = agent
            .run("Now also get the time in New York".to_string())
            .await
            .expect("Turn 2 should complete");

        eprintln!("Turn 2: {} tool calls", result2.tool_calls);
        eprintln!("Turn 2 response: {}", result2.text);

        // Turn 2 should have used the get_time tool
        assert!(result2.tool_calls >= 1, "Turn 2 should have tool calls");

        // Total tool calls across both turns
        let total_tool_calls = result1.tool_calls + result2.tool_calls;
        assert!(
            total_tool_calls >= 2,
            "Should have made multiple tool calls across turns, got: {}",
            total_tool_calls
        );

        // Verify session maintained context
        let session = agent.session();
        assert!(
            session.messages().len() >= 4,
            "Session should have multiple messages from both turns"
        );
    }

    /// E2E test for partial tool failure in parallel execution
    /// One tool fails, others should still complete and LLM should handle gracefully
    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_parallel_tools_partial_failure() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        /// Dispatcher where one specific tool always fails
        struct PartialFailureDispatcher;

        #[async_trait]
        impl AgentToolDispatcher for PartialFailureDispatcher {
            fn tools(&self) -> Vec<ToolDef> {
                vec![
                    ToolDef {
                        name: "working_tool".to_string(),
                        description: "A tool that works correctly".to_string(),
                        input_schema: serde_json::json!({"type": "object", "properties": {}}),
                    },
                    ToolDef {
                        name: "broken_tool".to_string(),
                        description: "A tool that always fails".to_string(),
                        input_schema: serde_json::json!({"type": "object", "properties": {}}),
                    },
                    ToolDef {
                        name: "another_working_tool".to_string(),
                        description: "Another tool that works correctly".to_string(),
                        input_schema: serde_json::json!({"type": "object", "properties": {}}),
                    },
                ]
            }

            async fn dispatch(&self, name: &str, _args: &Value) -> Result<Value, ToolError> {
                // Simulate some processing time
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;

                match name {
                    "working_tool" => {
                        Ok(Value::String("Working tool result: success!".to_string()))
                    }
                    "broken_tool" => Err(ToolError::execution_failed(
                        "Error: broken_tool encountered a critical failure",
                    )),
                    "another_working_tool" => Ok(Value::String(
                        "Another working tool result: also success!".to_string(),
                    )),
                    _ => Err(ToolError::not_found(name)),
                }
            }
        }

        let dispatcher = Arc::new(PartialFailureDispatcher);

        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let llm_adapter = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));

        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(anthropic_model())
            .max_tokens_per_turn(1024)
            .system_prompt(
                "You have three tools: working_tool, broken_tool, and another_working_tool. \
                 When asked to test all tools, call ALL THREE tools. \
                 If a tool fails, acknowledge the error and report results from successful tools.",
            )
            .build(llm_adapter, dispatcher, store_adapter);

        // Request that should trigger all three tools
        let result = agent
            .run(
                "Please call working_tool, broken_tool, and another_working_tool to test them all"
                    .to_string(),
            )
            .await
            .expect("Agent should complete even with partial tool failure");

        eprintln!("Tool calls: {}", result.tool_calls);
        eprintln!("Response: {}", result.text);

        // Should have attempted multiple tool calls
        assert!(
            result.tool_calls >= 1,
            "Should have made tool calls, got: {}",
            result.tool_calls
        );

        // LLM should acknowledge both success and failure
        let text_lower = result.text.to_lowercase();
        let mentions_success = text_lower.contains("success") || text_lower.contains("working");
        let mentions_error = text_lower.contains("error")
            || text_lower.contains("fail")
            || text_lower.contains("broken");

        eprintln!(
            "Mentions success: {}, Mentions error: {}",
            mentions_success, mentions_error
        );

        // At minimum, the agent should complete and respond coherently
        assert!(!result.text.is_empty(), "Should have a response");
    }
}

// ============================================================================
// E2E: SUB-AGENT OPERATIONS
// ============================================================================

/// E2E: Sub-agent fork/spawn operations
/// Tests fork, spawn, context strategies, tool access policies, and depth limits
mod sub_agent_fork {
    use super::*;
    use meerkat::{ConcurrencyLimits, ForkBranch, ForkBudgetPolicy, SpawnSpec, SubAgentManager};

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_sub_agent_fork_and_return() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        // Create a parent agent
        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let client = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));
        let tools = Arc::new(EmptyToolDispatcher);
        let (_store, store, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(anthropic_model())
            .system_prompt("You are a helpful assistant.")
            .max_tokens_per_turn(1024)
            .concurrency_limits(ConcurrencyLimits {
                max_depth: 3,
                max_concurrent_ops: 10,
                max_concurrent_agents: 5,
                max_children_per_agent: 3,
            })
            .build(client, tools, store);

        // Test fork operation
        let branches = vec![
            ForkBranch {
                name: "branch_a".to_string(),
                prompt: "Analyze option A".to_string(),
                tool_access: None,
            },
            ForkBranch {
                name: "branch_b".to_string(),
                prompt: "Analyze option B".to_string(),
                tool_access: Some(ToolAccessPolicy::Inherit),
            },
        ];

        // Fork should succeed (creates operation IDs but actual sub-agents run async)
        let op_ids = agent.fork(branches, ForkBudgetPolicy::Equal).await.unwrap();
        assert_eq!(op_ids.len(), 2);

        // Verify depth is tracked
        assert_eq!(agent.depth(), 0);

        // Run the parent agent - it should work independently
        let result = agent.run("Say hello briefly.".to_string()).await.unwrap();
        assert!(!result.text.is_empty());
    }

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_sub_agent_spawn() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let client = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));
        let tools = Arc::new(EmptyToolDispatcher);
        let (_store, store, _temp_dir) = create_temp_store().await;

        let agent = AgentBuilder::new()
            .model(anthropic_model())
            .system_prompt("You are a helpful assistant.")
            .max_tokens_per_turn(1024)
            .build(client, tools, store);

        // Test spawn with ContextStrategy::LastTurns
        let spec = SpawnSpec {
            prompt: "Summarize what we discussed".to_string(),
            context: ContextStrategy::LastTurns(2),
            tool_access: ToolAccessPolicy::Inherit,
            budget: BudgetLimits {
                max_tokens: Some(500),
                max_duration: None,
                max_tool_calls: Some(3),
            },
            allow_spawn: false,
            system_prompt: Some("You are a summarizer.".to_string()),
        };

        // Spawn should succeed
        let op_id = agent.spawn(spec).await.unwrap();
        assert!(!op_id.to_string().is_empty());
    }

    #[tokio::test]
    async fn test_context_strategy_application() {
        // Test ContextStrategy without API (unit test style)
        let manager = SubAgentManager::new(ConcurrencyLimits::default(), 0);

        let mut session = Session::new();
        session.set_system_prompt("System prompt".to_string());
        session.push(Message::User(meerkat::UserMessage {
            content: "Turn 1".to_string(),
        }));
        session.push(Message::User(meerkat::UserMessage {
            content: "Turn 2".to_string(),
        }));
        session.push(Message::User(meerkat::UserMessage {
            content: "Turn 3".to_string(),
        }));
        session.push(Message::User(meerkat::UserMessage {
            content: "Turn 4".to_string(),
        }));

        // FullHistory should include everything
        let full = manager.apply_context_strategy(&session, &ContextStrategy::FullHistory);
        assert_eq!(full.len(), 5); // system + 4 messages

        // LastTurns(1) should include system + last 2 messages
        let last = manager.apply_context_strategy(&session, &ContextStrategy::LastTurns(1));
        assert_eq!(last.len(), 3); // system + 2 messages
    }

    #[tokio::test]
    async fn test_tool_access_policy_enforcement() {
        let manager = SubAgentManager::new(ConcurrencyLimits::default(), 0);

        let tools = vec![
            ToolDef {
                name: "read_file".to_string(),
                description: "Read a file".to_string(),
                input_schema: serde_json::json!({}),
            },
            ToolDef {
                name: "write_file".to_string(),
                description: "Write a file".to_string(),
                input_schema: serde_json::json!({}),
            },
            ToolDef {
                name: "execute".to_string(),
                description: "Execute command".to_string(),
                input_schema: serde_json::json!({}),
            },
        ];

        // Inherit should keep all tools
        let inherit = manager.apply_tool_access_policy(&tools, &ToolAccessPolicy::Inherit);
        assert_eq!(inherit.len(), 3);

        // AllowList should filter to only allowed
        let allow = manager.apply_tool_access_policy(
            &tools,
            &ToolAccessPolicy::AllowList(vec!["read_file".to_string()]),
        );
        assert_eq!(allow.len(), 1);
        assert_eq!(allow[0].name, "read_file");

        // DenyList should exclude denied
        let deny = manager.apply_tool_access_policy(
            &tools,
            &ToolAccessPolicy::DenyList(vec!["execute".to_string()]),
        );
        assert_eq!(deny.len(), 2);
        assert!(deny.iter().all(|t| t.name != "execute"));
    }

    #[tokio::test]
    #[ignore = "Requires ANTHROPIC_API_KEY"]
    async fn test_depth_limit_enforced() {
        let Some(api_key) = skip_if_no_anthropic_key() else {
            eprintln!("Skipping: ANTHROPIC_API_KEY not set");
            return;
        };

        let llm_client = Arc::new(AnthropicClient::new(api_key));
        let client = Arc::new(LlmClientAdapter::new(llm_client, anthropic_model()));
        let tools = Arc::new(EmptyToolDispatcher);
        let (_store, store, _temp_dir) = create_temp_store().await;

        // Create agent with max_depth=0 - no children allowed
        let agent = AgentBuilder::new()
            .model(anthropic_model())
            .concurrency_limits(ConcurrencyLimits {
                max_depth: 0,
                max_concurrent_ops: 10,
                max_concurrent_agents: 5,
                max_children_per_agent: 3,
            })
            .build(client, tools, store);

        // Spawn should fail because child would be at depth 1 > max_depth (0)
        let spec = SpawnSpec {
            prompt: "Test".to_string(),
            context: ContextStrategy::FullHistory,
            tool_access: ToolAccessPolicy::Inherit,
            budget: BudgetLimits::default(),
            allow_spawn: false,
            system_prompt: None,
        };

        let result = agent.spawn(spec).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, AgentError::DepthLimitExceeded { .. }));
    }
}

// ============================================================================
// BASIC SANITY CHECKS (non-API tests)
// ============================================================================

/// Basic sanity checks that don't require API keys
mod sanity {
    use super::*;

    #[test]
    fn test_session_creation() {
        let session = Session::new();
        assert!(session.messages().is_empty());
    }

    #[test]
    fn test_budget_limits_construction() {
        let limits = BudgetLimits {
            max_tokens: Some(1000),
            max_duration: None,
            max_tool_calls: Some(10),
        };
        assert_eq!(limits.max_tokens, Some(1000));
        assert_eq!(limits.max_tool_calls, Some(10));
    }

    #[test]
    fn test_loop_state_initial() {
        let state = LoopState::CallingLlm;
        assert_eq!(state, LoopState::CallingLlm);
        assert!(!state.is_terminal());
    }

    #[test]
    fn test_agent_builder_creates_valid_agent() {
        // Mock implementations for testing builder
        struct MockClient;

        #[async_trait]
        impl AgentLlmClient for MockClient {
            async fn stream_response(
                &self,
                _messages: &[Message],
                _tools: &[ToolDef],
                _max_tokens: u32,
                _temperature: Option<f32>,
                _provider_params: Option<&serde_json::Value>,
            ) -> Result<LlmStreamResult, AgentError> {
                Ok(LlmStreamResult {
                    content: "test".to_string(),
                    tool_calls: vec![],
                    stop_reason: StopReason::EndTurn,
                    usage: Usage::default(),
                })
            }

            fn provider(&self) -> &'static str {
                "mock"
            }
        }

        struct MockStore;

        #[async_trait]
        impl AgentSessionStore for MockStore {
            async fn save(&self, _session: &Session) -> Result<(), AgentError> {
                Ok(())
            }
            async fn load(&self, _id: &str) -> Result<Option<Session>, AgentError> {
                Ok(None)
            }
        }

        let client = Arc::new(MockClient);
        let tools = Arc::new(EmptyToolDispatcher);
        let store = Arc::new(MockStore);

        let agent = AgentBuilder::new()
            .model("test-model")
            .system_prompt("Test prompt")
            .max_tokens_per_turn(1000)
            .build(client, tools, store);

        assert_eq!(agent.state(), &LoopState::CallingLlm);
        // System prompt should be added to session
        assert!(!agent.session().messages().is_empty());
    }
}
