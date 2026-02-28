#![cfg(feature = "integration-real-tests")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! E2E smoke tests for the Meerkat native Rust SDK.
//!
//! These tests verify compound, realistic scenario-based workflows through
//! the AgentFactory path (and AgentBuilder where injection is needed).
//!
//! Each test requires API keys and makes real API calls. When keys are missing,
//! tests skip gracefully with an informational message.
//!
//! Run with:
//!   cargo test -p meerkat --test e2e_smoke -- --ignored --test-threads=1

use async_trait::async_trait;
use futures::StreamExt;
use meerkat::*;
use meerkat_core::ToolCallView;
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
        tools: &[Arc<ToolDef>],
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
                    LlmEvent::TextDelta { delta, .. } => {
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
                    LlmEvent::Done { outcome } => match outcome {
                        LlmDoneOutcome::Success { stop_reason: sr } => {
                            stop_reason = sr;
                        }
                        LlmDoneOutcome::Error { error } => {
                            return Err(AgentError::llm(
                                self.client.provider(),
                                error.failure_reason(),
                                error.to_string(),
                            ));
                        }
                    },
                    LlmEvent::ReasoningDelta { .. } | LlmEvent::ReasoningComplete { .. } => {}
                },
                Err(e) => {
                    return Err(AgentError::llm(
                        self.client.provider(),
                        e.failure_reason(),
                        e.to_string(),
                    ));
                }
            }
        }

        // Complete any buffered tool calls
        for (_, buffer) in tool_call_buffers {
            if let Some(tc) = buffer.try_complete()
                && !tool_calls.iter().any(|t| t.id == tc.id)
            {
                tool_calls.push(tc);
            }
        }

        let mut blocks = Vec::new();
        if !content.is_empty() {
            blocks.push(meerkat_core::AssistantBlock::Text {
                text: content,
                meta: None,
            });
        }
        for tc in tool_calls {
            let args_raw = serde_json::value::RawValue::from_string(
                serde_json::to_string(&tc.args).unwrap_or_else(|_| "{}".to_string()),
            )
            .unwrap_or_else(|_| {
                serde_json::value::RawValue::from_string("{}".to_string()).unwrap()
            });
            blocks.push(meerkat_core::AssistantBlock::ToolUse {
                id: tc.id,
                name: tc.name,
                args: args_raw,
                meta: None,
            });
        }
        Ok(LlmStreamResult::new(blocks, stop_reason, usage))
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
            .map_err(|e| AgentError::StoreError(format!("Invalid session ID: {e}")))?;

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
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolResult, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

fn first_env(vars: &[&str]) -> Option<String> {
    for name in vars {
        if let Ok(value) = std::env::var(name) {
            return Some(value);
        }
    }
    None
}

fn anthropic_api_key() -> Option<String> {
    first_env(&["RKAT_ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY"])
}

#[allow(dead_code)]
fn openai_api_key() -> Option<String> {
    first_env(&["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"])
}

#[allow(dead_code)]
fn gemini_api_key() -> Option<String> {
    first_env(&["RKAT_GEMINI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"])
}

/// Get the model to use for smoke tests (cheaper model by default).
fn smoke_model() -> String {
    std::env::var("SMOKE_MODEL").unwrap_or_else(|_| "claude-sonnet-4-5".to_string())
}

/// Create a store adapter using JsonlStore with a temp directory.
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
// SCENARIO 1: Multi-provider round-robin
// ============================================================================

mod scenario_01_multi_provider {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_multi_provider_round_robin() {
        let mut ran_any = false;

        // --- Anthropic ---
        if let Some(api_key) = anthropic_api_key() {
            eprintln!(
                "[scenario 1] Testing Anthropic with model {}",
                smoke_model()
            );
            let temp_dir = TempDir::new().unwrap();
            let factory = AgentFactory::new(temp_dir.path().join("sessions"));
            let config = Config::default();
            let build_config = AgentBuildConfig::new(smoke_model());

            let mut agent = factory
                .build_agent(build_config, &config)
                .await
                .expect("Anthropic agent should build");

            let result = agent
                .run("What is 2+2? Answer with just the number.".to_string())
                .await
                .expect("Anthropic agent run should succeed");

            assert!(
                !result.text.is_empty(),
                "Anthropic response should not be empty"
            );
            assert!(
                result.text.contains('4') || result.text.to_lowercase().contains("four"),
                "Anthropic response should contain 4: {}",
                result.text
            );
            eprintln!("[scenario 1] Anthropic OK: {}", result.text.trim());
            ran_any = true;

            // Drop to avoid holding the api_key borrow
            drop(api_key);
        } else {
            eprintln!("[scenario 1] Skipping Anthropic: no API key");
        }

        // --- OpenAI ---
        #[cfg(feature = "openai")]
        if let Some(_api_key) = openai_api_key() {
            let openai_model = "gpt-5.2".to_string();
            eprintln!("[scenario 1] Testing OpenAI with model {openai_model}");
            let temp_dir = TempDir::new().unwrap();
            let factory = AgentFactory::new(temp_dir.path().join("sessions"));
            let config = Config::default();
            let mut build_config = AgentBuildConfig::new(openai_model.clone());
            build_config.provider = Some(Provider::OpenAI);

            let mut agent = factory
                .build_agent(build_config, &config)
                .await
                .expect("OpenAI agent should build");

            let result = agent
                .run("What is 2+2? Answer with just the number.".to_string())
                .await
                .expect("OpenAI agent run should succeed");

            assert!(
                !result.text.is_empty(),
                "OpenAI response should not be empty"
            );
            assert!(
                result.text.contains('4') || result.text.to_lowercase().contains("four"),
                "OpenAI response should contain 4: {}",
                result.text
            );
            eprintln!("[scenario 1] OpenAI OK: {}", result.text.trim());
            ran_any = true;
        } else {
            eprintln!("[scenario 1] Skipping OpenAI: no API key");
        }

        // --- Gemini ---
        #[cfg(feature = "gemini")]
        if let Some(_api_key) = gemini_api_key() {
            let gemini_model = "gemini-3-flash-preview".to_string();
            eprintln!("[scenario 1] Testing Gemini with model {gemini_model}");
            let temp_dir = TempDir::new().unwrap();
            let factory = AgentFactory::new(temp_dir.path().join("sessions"));
            let config = Config::default();
            let mut build_config = AgentBuildConfig::new(gemini_model.clone());
            build_config.provider = Some(Provider::Gemini);

            let mut agent = factory
                .build_agent(build_config, &config)
                .await
                .expect("Gemini agent should build");

            let result = agent
                .run("What is 2+2? Answer with just the number.".to_string())
                .await
                .expect("Gemini agent run should succeed");

            assert!(
                !result.text.is_empty(),
                "Gemini response should not be empty"
            );
            assert!(
                result.text.contains('4') || result.text.to_lowercase().contains("four"),
                "Gemini response should contain 4: {}",
                result.text
            );
            eprintln!("[scenario 1] Gemini OK: {}", result.text.trim());
            ran_any = true;
        } else {
            eprintln!("[scenario 1] Skipping Gemini: no API key");
        }

        assert!(
            ran_any,
            "At least one provider API key must be available for scenario 1"
        );
    }
}

// ============================================================================
// SCENARIO 2: Tool-driven shell
// ============================================================================

mod scenario_02_tool_driven_shell {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_tool_driven_shell() {
        let Some(_api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 2: missing ANTHROPIC_API_KEY");
            return;
        };

        let temp_dir = TempDir::new().unwrap();
        let factory = AgentFactory::new(temp_dir.path().join("sessions"))
            .builtins(true)
            .shell(true)
            .project_root(temp_dir.path());

        let config = Config::default();
        let mut build_config = AgentBuildConfig::new(smoke_model());
        build_config.system_prompt = Some(
            "You are a helpful assistant with shell access. Execute commands as asked.".to_string(),
        );

        let mut agent = factory
            .build_agent(build_config, &config)
            .await
            .expect("Shell agent should build");

        let result = agent
            .run(
                "Use the shell tool to run: echo 'hello meerkat smoke'. \
                 Then tell me what the output was."
                    .to_string(),
            )
            .await
            .expect("Shell agent run should succeed");

        eprintln!(
            "[scenario 2] tool_calls={}, text={}",
            result.tool_calls,
            result.text.trim()
        );

        assert!(
            result.tool_calls >= 1,
            "Should have called shell tool at least once, got {}",
            result.tool_calls
        );
        assert!(
            result.text.to_lowercase().contains("hello meerkat")
                || result.text.to_lowercase().contains("hello")
                || result.text.to_lowercase().contains("meerkat"),
            "Response should mention the echo output: {}",
            result.text
        );
    }
}

// ============================================================================
// SCENARIO 3: Structured output + tools
// ============================================================================

mod scenario_03_structured_output {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_structured_output_with_tools() {
        let Some(_api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 3: missing ANTHROPIC_API_KEY");
            return;
        };

        let temp_dir = TempDir::new().unwrap();
        let factory = AgentFactory::new(temp_dir.path().join("sessions"))
            .builtins(true)
            .shell(true)
            .project_root(temp_dir.path());

        let config = Config::default();

        // Define a schema: { files: [{ name: string, size: number }] }
        let schema_json = serde_json::json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "size": { "type": "number" }
                        },
                        "required": ["name", "size"]
                    }
                }
            },
            "required": ["files"]
        });
        let schema = MeerkatSchema::new(schema_json).expect("Schema should be valid");
        let output_schema = OutputSchema {
            schema,
            name: Some("file_listing".to_string()),
            strict: false,
            compat: SchemaCompat::Lossy,
            format: SchemaFormat::default(),
        };

        let mut build_config = AgentBuildConfig::new(smoke_model());
        build_config.output_schema = Some(output_schema);
        build_config.system_prompt = Some(
            "You are a helpful assistant with shell access. \
             List files as instructed."
                .to_string(),
        );

        let mut agent = factory
            .build_agent(build_config, &config)
            .await
            .expect("Structured output agent should build");

        // Create some test files so there is something to list
        std::fs::write(temp_dir.path().join("alpha.txt"), "hello").unwrap();
        std::fs::write(temp_dir.path().join("beta.txt"), "world").unwrap();

        let result = agent
            .run(
                "List the files in the current directory using the shell tool, \
                 then report what you found."
                    .to_string(),
            )
            .await
            .expect("Structured output run should succeed");

        eprintln!(
            "[scenario 3] tool_calls={}, structured_output={:?}",
            result.tool_calls,
            result.structured_output.is_some()
        );

        // The agent should have used tools and produced structured output
        assert!(
            result.tool_calls > 0,
            "Should have called shell tool, got {}",
            result.tool_calls
        );

        let so = result
            .structured_output
            .as_ref()
            .expect("structured_output must be present — extraction turn is mandatory");

        let files = so
            .get("files")
            .and_then(|f| f.as_array())
            .expect("Structured output must have a 'files' array");
        assert!(!files.is_empty(), "Files array must not be empty: {so:?}");

        for file in files {
            assert!(
                file.get("name").is_some(),
                "Each file must have a 'name' field: {file:?}"
            );
            assert!(
                file.get("size").is_some(),
                "Each file must have a 'size' field: {file:?}"
            );
        }
        eprintln!(
            "[scenario 3] Structured output validated: {} files",
            files.len()
        );
    }
}

// ============================================================================
// SCENARIO 4: Sub-agent spawn
// ============================================================================

mod scenario_04_sub_agent {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_sub_agent_spawn() {
        let Some(_api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 4: missing ANTHROPIC_API_KEY");
            return;
        };

        // Use AgentFactory with subagents enabled — the agent gets the
        // agent_spawn/agent_fork/agent_status/agent_list tools and must
        // actually use them to complete the prompt.
        let temp_dir = TempDir::new().unwrap();
        let factory = AgentFactory::new(temp_dir.path().join("sessions"))
            .builtins(true)
            .subagents(true)
            .project_root(temp_dir.path());

        let config = Config::default();
        let mut build_config = AgentBuildConfig::new(smoke_model());
        build_config.system_prompt = Some(
            "You are an orchestrator. When asked to analyze something, \
             use agent_spawn to delegate sub-tasks. After spawning, \
             use agent_status to check results. Be brief."
                .to_string(),
        );

        let mut agent = factory
            .build_agent(build_config, &config)
            .await
            .expect("Sub-agent factory should build");

        let result = agent
            .run(
                "Spawn a sub-agent to answer: what is 2+2? \
                 Then check its status with agent_status and tell me the result."
                    .to_string(),
            )
            .await
            .expect("Sub-agent orchestration should succeed");

        eprintln!(
            "[scenario 4] tool_calls={}, turns={}, text={}",
            result.tool_calls,
            result.turns,
            &result.text[..result.text.len().min(120)]
        );

        assert!(
            result.tool_calls >= 2,
            "Should have called agent_spawn + agent_status (at least 2 tool calls), got {}",
            result.tool_calls
        );
        let text_lower = result.text.to_lowercase();
        assert!(
            text_lower.contains('4') || text_lower.contains("four"),
            "Should relay the sub-agent's answer (4), got: {}",
            result.text
        );
    }
}

// ============================================================================
// SCENARIO 5: Multi-turn conversation with context recall
// ============================================================================

mod scenario_05_multi_turn {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_multi_turn_context_recall() {
        let Some(_api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 5: missing ANTHROPIC_API_KEY");
            return;
        };

        let temp_dir = TempDir::new().unwrap();
        let factory = AgentFactory::new(temp_dir.path().join("sessions"));
        let config = Config::default();

        let mut build_config = AgentBuildConfig::new(smoke_model());
        build_config.system_prompt =
            Some("You are a helpful assistant. Keep your responses brief and precise.".to_string());

        let mut agent = factory
            .build_agent(build_config, &config)
            .await
            .expect("Multi-turn agent should build");

        // Turn 1: Establish name and number
        let result1 = agent
            .run("My name is SmokeBot and my favorite number is 7.".to_string())
            .await
            .expect("Turn 1 should succeed");
        assert!(!result1.text.is_empty());
        eprintln!("[scenario 5] Turn 1: {}", result1.text.trim());

        // Turn 2: Unrelated question (to push context)
        let result2 = agent
            .run("What is the capital of France?".to_string())
            .await
            .expect("Turn 2 should succeed");
        assert!(!result2.text.is_empty());
        eprintln!("[scenario 5] Turn 2: {}", result2.text.trim());

        // Turn 3: Arithmetic with the favorite number
        let result3 = agent
            .run("Now multiply my favorite number by 6. What is the result?".to_string())
            .await
            .expect("Turn 3 should succeed");
        assert!(
            result3.text.contains("42"),
            "Turn 3 should mention 42 (7 * 6): {}",
            result3.text
        );
        eprintln!("[scenario 5] Turn 3: {}", result3.text.trim());

        // Turn 4: Another unrelated question
        let result4 = agent
            .run("What is 100 + 200?".to_string())
            .await
            .expect("Turn 4 should succeed");
        assert!(!result4.text.is_empty());
        eprintln!("[scenario 5] Turn 4: {}", result4.text.trim());

        // Turn 5: Recall name and number from turn 1
        let result5 = agent
            .run("Remind me: what is my name and what was my favorite number?".to_string())
            .await
            .expect("Turn 5 should succeed");
        let text5 = result5.text.to_lowercase();
        assert!(
            text5.contains("smokebot") || text5.contains("smoke"),
            "Turn 5 should recall the name SmokeBot: {}",
            result5.text
        );
        assert!(
            text5.contains('7') || text5.contains("seven"),
            "Turn 5 should recall the number 7: {}",
            result5.text
        );
        eprintln!("[scenario 5] Turn 5: {}", result5.text.trim());
    }
}

// ============================================================================
// SCENARIO 6: Hooks pipeline (observe + rewrite + guardrail)
// ============================================================================

mod scenario_06_hooks {
    use super::*;
    use meerkat_hooks::{DefaultHookEngine, RuntimeHookResponse};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_hooks_pipeline() {
        let Some(api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 6: missing ANTHROPIC_API_KEY");
            return;
        };

        // Track whether each hook type was invoked
        let observer_called = Arc::new(AtomicBool::new(false));
        let guardrail_called = Arc::new(AtomicBool::new(false));
        let rewriter_called = Arc::new(AtomicBool::new(false));

        let observer_called_clone = observer_called.clone();
        let guardrail_called_clone = guardrail_called.clone();
        let rewriter_called_clone = rewriter_called.clone();

        // Build a HooksConfig with three hook entries:
        // 1. Observer on RunStarted
        // 2. Guardrail on PreLlmRequest (always Allow)
        // 3. Rewriter on PostLlmResponse (appends " [REVIEWED]")
        let hooks_config = HooksConfig {
            entries: vec![
                HookEntryConfig {
                    id: HookId::new("observer"),
                    enabled: true,
                    point: HookPoint::RunStarted,
                    mode: HookExecutionMode::Foreground,
                    capability: HookCapability::Observe,
                    priority: 100,
                    failure_policy: None,
                    timeout_ms: None,
                    runtime: HookRuntimeConfig::new(
                        "in_process",
                        Some(serde_json::json!({"name": "observer"})),
                    )
                    .unwrap(),
                },
                HookEntryConfig {
                    id: HookId::new("guardrail"),
                    enabled: true,
                    point: HookPoint::PreLlmRequest,
                    mode: HookExecutionMode::Foreground,
                    capability: HookCapability::Guardrail,
                    priority: 90,
                    failure_policy: None,
                    timeout_ms: None,
                    runtime: HookRuntimeConfig::new(
                        "in_process",
                        Some(serde_json::json!({"name": "guardrail"})),
                    )
                    .unwrap(),
                },
                HookEntryConfig {
                    id: HookId::new("rewriter"),
                    enabled: true,
                    point: HookPoint::PostLlmResponse,
                    mode: HookExecutionMode::Foreground,
                    capability: HookCapability::Rewrite,
                    priority: 80,
                    failure_policy: None,
                    timeout_ms: None,
                    runtime: HookRuntimeConfig::new(
                        "in_process",
                        Some(serde_json::json!({"name": "rewriter"})),
                    )
                    .unwrap(),
                },
            ],
            ..Default::default()
        };

        let engine = DefaultHookEngine::new(hooks_config);

        // Register observer handler
        engine
            .register_in_process_handler(
                "observer",
                Arc::new(move |_invocation| {
                    observer_called_clone.store(true, Ordering::SeqCst);
                    Box::pin(async {
                        Ok(RuntimeHookResponse {
                            decision: None,
                            patches: Vec::new(),
                        })
                    })
                }),
            )
            .await;

        // Register guardrail handler (always allows)
        engine
            .register_in_process_handler(
                "guardrail",
                Arc::new(move |_invocation| {
                    guardrail_called_clone.store(true, Ordering::SeqCst);
                    Box::pin(async {
                        Ok(RuntimeHookResponse {
                            decision: Some(HookDecision::Allow),
                            patches: Vec::new(),
                        })
                    })
                }),
            )
            .await;

        // Register rewriter handler (appends " [REVIEWED]" to assistant text)
        engine
            .register_in_process_handler(
                "rewriter",
                Arc::new(move |invocation| {
                    rewriter_called_clone.store(true, Ordering::SeqCst);
                    let original_text = invocation
                        .llm_response
                        .as_ref()
                        .map(|r| r.assistant_text.clone())
                        .unwrap_or_default();
                    Box::pin(async move {
                        Ok(RuntimeHookResponse {
                            decision: None,
                            patches: vec![HookPatch::AssistantText {
                                text: format!("{original_text} [REVIEWED]"),
                            }],
                        })
                    })
                }),
            )
            .await;

        let hook_engine: Arc<dyn HookEngine> = Arc::new(engine);

        // Build agent with the hook engine
        let llm_client = Arc::new(AnthropicClient::new(api_key).unwrap());
        let llm_adapter = Arc::new(self::LlmClientAdapter::new(llm_client, smoke_model()));
        let tools = Arc::new(EmptyToolDispatcher);
        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(smoke_model())
            .max_tokens_per_turn(256)
            .system_prompt("You are a helpful assistant. Keep responses brief.")
            .with_hook_engine(hook_engine)
            .build(llm_adapter, tools, store_adapter)
            .await;

        let result = agent
            .run("Say hello.".to_string())
            .await
            .expect("Hooked agent run should succeed");

        eprintln!("[scenario 6] Result text: {}", result.text.trim());

        // Verify all three hook types were invoked
        assert!(
            observer_called.load(Ordering::SeqCst),
            "Observer hook should have been called"
        );
        assert!(
            guardrail_called.load(Ordering::SeqCst),
            "Guardrail hook should have been called"
        );
        assert!(
            rewriter_called.load(Ordering::SeqCst),
            "Rewriter hook should have been called"
        );

        // Verify the rewrite was applied
        assert!(
            result.text.contains("[REVIEWED]"),
            "Result text should end with [REVIEWED] after rewrite: {}",
            result.text
        );
    }
}

// ============================================================================
// SCENARIO 7: Session persistence + resume
// ============================================================================

mod scenario_07_session_resume {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_session_persist_and_resume() {
        let Some(_api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 7: missing ANTHROPIC_API_KEY");
            return;
        };

        let temp_dir = TempDir::new().unwrap();
        let store_path = temp_dir.path().join("sessions");

        // Phase 1: Create agent, establish context, save session
        let session_id = {
            let factory = AgentFactory::new(store_path.clone());
            let config = Config::default();
            let mut build_config = AgentBuildConfig::new(smoke_model());
            build_config.system_prompt = Some(
                "You are a helpful assistant. Remember everything the user tells you.".to_string(),
            );

            let mut agent = factory
                .build_agent(build_config, &config)
                .await
                .expect("Phase 1 agent should build");

            let result = agent
                .run("My secret code is ALPHA-7. Remember it.".to_string())
                .await
                .expect("Phase 1 turn should succeed");

            eprintln!("[scenario 7] Phase 1: {}", result.text.trim());
            result.session_id
        };

        // Phase 2: Create a NEW agent, resume the session, verify context
        {
            let store = JsonlStore::new(store_path.clone());
            store.init().await.expect("Store init");
            let saved_session = store
                .load(&session_id)
                .await
                .expect("Load should succeed")
                .expect("Session should exist");

            // Verify session metadata survived serialization
            let metadata = saved_session.session_metadata();
            if let Some(meta) = metadata {
                assert_eq!(meta.model, smoke_model(), "Model should be preserved");
                eprintln!(
                    "[scenario 7] Preserved metadata: model={}, provider={:?}",
                    meta.model, meta.provider
                );
            }

            let factory = AgentFactory::new(store_path.clone());
            let config = Config::default();
            let mut build_config = AgentBuildConfig::new(smoke_model());
            build_config.resume_session = Some(saved_session);

            let mut agent = factory
                .build_agent(build_config, &config)
                .await
                .expect("Phase 2 agent should build");

            let result = agent
                .run("What was my secret code?".to_string())
                .await
                .expect("Phase 2 turn should succeed");

            eprintln!("[scenario 7] Phase 2: {}", result.text.trim());
            assert!(
                result.text.contains("ALPHA") || result.text.contains('7'),
                "Resumed agent should remember the secret code: {}",
                result.text
            );
        }
    }
}

// ============================================================================
// SCENARIO 8: Comms exchange (two-agent TCP)
// ============================================================================

#[cfg(feature = "comms")]
mod scenario_08_comms {
    use super::*;
    use meerkat_comms::agent::{
        CommsAgent, CommsManager, CommsManagerConfig, CommsToolDispatcher, spawn_tcp_listener,
    };
    use meerkat_comms::{CommsConfig, Keypair, TrustedPeer, TrustedPeers};
    use tokio::sync::RwLock;

    /// Create a pair of agents that can communicate with each other.
    async fn create_agent_pair(
        api_key: &str,
    ) -> (
        CommsAgent<
            LlmClientAdapter<AnthropicClient>,
            CommsToolDispatcher,
            SessionStoreAdapter<JsonlStore>,
        >,
        CommsAgent<
            LlmClientAdapter<AnthropicClient>,
            CommsToolDispatcher,
            SessionStoreAdapter<JsonlStore>,
        >,
        meerkat_comms::agent::ListenerHandle,
        meerkat_comms::agent::ListenerHandle,
        TempDir,
        TempDir,
    ) {
        let keypair_a = Keypair::generate();
        let keypair_b = Keypair::generate();
        let pubkey_a = keypair_a.public_key();
        let pubkey_b = keypair_b.public_key();

        let listener_a = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_a = listener_a.local_addr().unwrap();
        drop(listener_a);

        let listener_b = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_b = listener_b.local_addr().unwrap();
        drop(listener_b);

        let trusted_for_a = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "agent-b".to_string(),
                pubkey: pubkey_b,
                addr: format!("tcp://{addr_b}"),
                meta: meerkat_comms::PeerMeta::default(),
            }],
        };

        let trusted_for_b = TrustedPeers {
            peers: vec![TrustedPeer {
                name: "agent-a".to_string(),
                pubkey: pubkey_a,
                addr: format!("tcp://{addr_a}"),
                meta: meerkat_comms::PeerMeta::default(),
            }],
        };

        let config_a = CommsManagerConfig::with_keypair(keypair_a)
            .trusted_peers(trusted_for_a.clone())
            .comms_config(CommsConfig::default());
        let comms_manager_a = CommsManager::new(config_a).unwrap();

        let config_b = CommsManagerConfig::with_keypair(keypair_b)
            .trusted_peers(trusted_for_b.clone())
            .comms_config(CommsConfig::default());
        let comms_manager_b = CommsManager::new(config_b).unwrap();

        let trusted_a_shared = Arc::new(RwLock::new(trusted_for_a));
        let trusted_b_shared = Arc::new(RwLock::new(trusted_for_b));

        let handle_a = spawn_tcp_listener(
            &addr_a.to_string(),
            comms_manager_a.keypair_arc(),
            trusted_a_shared.clone(),
            comms_manager_a.inbox_sender().clone(),
        )
        .await
        .expect("Failed to start listener A");

        let handle_b = spawn_tcp_listener(
            &addr_b.to_string(),
            comms_manager_b.keypair_arc(),
            trusted_b_shared.clone(),
            comms_manager_b.inbox_sender().clone(),
        )
        .await
        .expect("Failed to start listener B");

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let llm_client_a = Arc::new(AnthropicClient::new(api_key.to_string()).unwrap());
        let llm_adapter_a = Arc::new(LlmClientAdapter::new(llm_client_a, smoke_model()));

        let llm_client_b = Arc::new(AnthropicClient::new(api_key.to_string()).unwrap());
        let llm_adapter_b = Arc::new(LlmClientAdapter::new(llm_client_b, smoke_model()));

        let tools_a = Arc::new(CommsToolDispatcher::new(
            comms_manager_a.router().clone(),
            trusted_a_shared,
        ));

        let tools_b = Arc::new(CommsToolDispatcher::new(
            comms_manager_b.router().clone(),
            trusted_b_shared,
        ));

        let (_store_a, store_adapter_a, temp_dir_a) = create_temp_store().await;
        let (_store_b, store_adapter_b, temp_dir_b) = create_temp_store().await;

        let agent_a_inner = AgentBuilder::new()
            .model(smoke_model())
            .max_tokens_per_turn(1024)
            .system_prompt(
                "You are Agent A. Use the comms tools `send` and `peers`. \
                 To message agent-b, call `send` with kind `peer_message`.",
            )
            .build(llm_adapter_a, tools_a, store_adapter_a)
            .await;

        let agent_b_inner = AgentBuilder::new()
            .model(smoke_model())
            .max_tokens_per_turn(1024)
            .system_prompt(
                "You are Agent B. You can receive messages from agent-a. \
                 Acknowledge any messages you receive.",
            )
            .build(llm_adapter_b, tools_b, store_adapter_b)
            .await;

        let agent_a = CommsAgent::new(agent_a_inner, comms_manager_a);
        let agent_b = CommsAgent::new(agent_b_inner, comms_manager_b);

        (agent_a, agent_b, handle_a, handle_b, temp_dir_a, temp_dir_b)
    }

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_comms_exchange() {
        let Some(api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 8: missing ANTHROPIC_API_KEY");
            return;
        };

        let (mut agent_a, mut agent_b, handle_a, handle_b, _temp_a, _temp_b) =
            create_agent_pair(&api_key).await;

        // Agent A sends a message to Agent B via comms tools.
        let result_a = agent_a
            .run(
                "Use the send tool to send a peer_message to agent-b with body \
                 exactly: Smoke test ping from Agent A"
                    .to_string(),
            )
            .await
            .expect("Agent A run should succeed");
        eprintln!(
            "[scenario 8] Agent A: tool_calls={}, text={}",
            result_a.tool_calls,
            result_a.text.trim()
        );
        assert!(
            result_a.tool_calls > 0,
            "Agent A should make at least one comms tool call"
        );

        // Give time for message delivery
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        // Agent B processes the incoming message
        let result_b = agent_b
            .run("Process your inbox and acknowledge what Agent A sent.".to_string())
            .await
            .expect("Agent B inbox processing should succeed");

        eprintln!("[scenario 8] Agent B: text={}", result_b.text.trim());
        let text_b_lower = result_b.text.to_lowercase();
        assert!(
            text_b_lower.contains("agent a")
                || text_b_lower.contains("ping")
                || text_b_lower.contains("smoke")
                || text_b_lower.contains("message")
                || text_b_lower.contains("received"),
            "Agent B should acknowledge the message: {}",
            result_b.text
        );

        handle_a.abort();
        handle_b.abort();
    }
}

// ============================================================================
// SCENARIO 9: Session service lifecycle (EphemeralSessionService)
// ============================================================================

mod scenario_09_session_service {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_session_service_lifecycle() {
        let Some(_api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 9: missing ANTHROPIC_API_KEY");
            return;
        };

        let temp_dir = TempDir::new().unwrap();
        let factory = AgentFactory::new(temp_dir.path().join("sessions"));
        let config = Config::default();

        // Build ephemeral session service
        let service = build_ephemeral_service(factory, config, 10);

        // 1. Create session (runs first turn)
        let create_req = CreateSessionRequest {
            model: smoke_model(),
            prompt: "Hello, I am testing the session service.".to_string(),
            system_prompt: Some("You are a helpful assistant. Be brief.".to_string()),
            max_tokens: Some(256),
            event_tx: None,
            host_mode: false,
            skill_references: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
            build: None,
            labels: None,
        };

        let create_result = service
            .create_session(create_req)
            .await
            .expect("create_session should succeed");

        let session_id = create_result.session_id.clone();
        eprintln!(
            "[scenario 9] Created session: {}, text: {}",
            session_id,
            create_result.text.trim()
        );
        assert!(
            !create_result.text.is_empty(),
            "First turn should produce text"
        );

        // 2. Read session
        let view = service
            .read(&session_id)
            .await
            .expect("read should succeed");
        assert_eq!(
            view.session_id(),
            &session_id,
            "Read should return the same session ID"
        );
        assert!(
            view.state.message_count >= 2,
            "Session should have at least system + user + assistant messages"
        );
        eprintln!(
            "[scenario 9] Read session: {} messages",
            view.state.message_count
        );

        // 3. Start follow-up turn
        let turn_req = StartTurnRequest {
            prompt: "What did I just say to you?".to_string(),
            event_tx: None,
            host_mode: false,
            skill_references: None,
            flow_tool_overlay: None,
        };

        let turn_result = service
            .start_turn(&session_id, turn_req)
            .await
            .expect("start_turn should succeed");

        eprintln!("[scenario 9] Follow-up turn: {}", turn_result.text.trim());
        assert!(
            !turn_result.text.is_empty(),
            "Follow-up should produce text"
        );

        // 4. List sessions (verify present)
        let summaries = service
            .list(SessionQuery::default())
            .await
            .expect("list should succeed");
        assert!(
            summaries.iter().any(|s| s.session_id == session_id),
            "Session should appear in list"
        );
        eprintln!("[scenario 9] List: {} sessions", summaries.len());

        // 5. Archive
        service
            .archive(&session_id)
            .await
            .expect("archive should succeed");
        eprintln!("[scenario 9] Archived session {session_id}");

        // 6. List again (verify absent)
        let summaries_after = service
            .list(SessionQuery::default())
            .await
            .expect("list after archive should succeed");
        assert!(
            !summaries_after.iter().any(|s| s.session_id == session_id),
            "Archived session should not appear in list"
        );
        eprintln!(
            "[scenario 9] Post-archive list: {} sessions",
            summaries_after.len()
        );
    }
}

// ============================================================================
// SCENARIO 10: Memory compaction + semantic recall
// ============================================================================

#[cfg(all(feature = "memory-store-session", feature = "session-compaction"))]
mod scenario_10_memory {
    use super::*;
    use meerkat_core::CompactionConfig;
    use meerkat_memory::SimpleMemoryStore;
    use meerkat_session::DefaultCompactor;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_memory_compaction_and_search() {
        let Some(api_key_val) = anthropic_api_key() else {
            eprintln!("Skipping scenario 10: missing ANTHROPIC_API_KEY");
            return;
        };

        // Use SimpleMemoryStore (keyword-matching, no HNSW/redb needed for test)
        let memory_store =
            Arc::new(SimpleMemoryStore::new()) as Arc<dyn meerkat_core::memory::MemoryStore>;

        // Low-threshold compactor to trigger compaction quickly
        let compactor_config = CompactionConfig {
            auto_compact_threshold: 100, // Very low: trigger after ~100 input tokens
            recent_turn_budget: 1,       // Keep only 1 recent turn
            max_summary_tokens: 256,
            min_turns_between_compactions: 1,
        };
        let compactor = Arc::new(DefaultCompactor::new(compactor_config))
            as Arc<dyn meerkat_core::compact::Compactor>;

        // Build memory_search tool dispatcher
        let memory_dispatcher =
            meerkat_memory::MemorySearchDispatcher::new(Arc::clone(&memory_store));
        let memory_tools: Arc<dyn AgentToolDispatcher> = Arc::new(memory_dispatcher);

        // Build agent with memory store + compactor + memory_search tool
        let llm_client = Arc::new(AnthropicClient::new(api_key_val).unwrap());
        let llm_adapter = Arc::new(self::LlmClientAdapter::new(llm_client, smoke_model()));
        let (_store, store_adapter, _temp_dir) = create_temp_store().await;

        let mut agent = AgentBuilder::new()
            .model(smoke_model())
            .max_tokens_per_turn(512)
            .system_prompt(
                "You are a helpful assistant with access to a memory_search tool. \
                 When asked to recall information from your memory, use the memory_search tool. \
                 Keep responses brief.",
            )
            .memory_store(Arc::clone(&memory_store))
            .compactor(compactor)
            .build(llm_adapter, memory_tools, store_adapter)
            .await;

        // Turn 1: Distinctive content
        let r1 = agent
            .run("The secret project codename is AURORA-7.".to_string())
            .await
            .expect("Turn 1 should succeed");
        eprintln!("[scenario 10] Turn 1: {}", r1.text.trim());

        // Turn 2: More distinctive content
        let r2 = agent
            .run("The project budget is exactly $42,000.".to_string())
            .await
            .expect("Turn 2 should succeed");
        eprintln!("[scenario 10] Turn 2: {}", r2.text.trim());

        // Turn 3: Even more content to push toward compaction threshold
        let r3 = agent
            .run("The project deadline is March 15th. The team lead is named Zara.".to_string())
            .await
            .expect("Turn 3 should succeed");
        eprintln!("[scenario 10] Turn 3: {}", r3.text.trim());

        // Turn 4: More context to trigger compaction
        let r4 = agent
            .run(
                "The project uses Rust and targets embedded systems. \
                 Key dependencies include tokio and serde."
                    .to_string(),
            )
            .await
            .expect("Turn 4 should succeed");
        eprintln!("[scenario 10] Turn 4: {}", r4.text.trim());

        // At this point, compaction may have fired, discarding early turns
        // and indexing their content in the memory store.
        //
        // Now ask the agent to use memory_search to find the codename.
        let r5 = agent
            .run(
                "Use the memory_search tool to search for information about 'project codename'. \
                 What codename did we discuss?"
                    .to_string(),
            )
            .await
            .expect("Turn 5 (memory search) should succeed");

        eprintln!(
            "[scenario 10] Turn 5 (memory search): tool_calls={}, text={}",
            r5.tool_calls,
            r5.text.trim()
        );

        // The memory_search tool must have been called. If compaction didn't
        // fire the test configuration is wrong (threshold too high).
        assert!(
            r5.tool_calls > 0,
            "memory_search tool must be called — compaction should have discarded \
             early turns and the agent should search memory to recall them. \
             Got 0 tool_calls. text={}",
            r5.text
        );
        eprintln!(
            "[scenario 10] memory_search tool was called ({} tool calls)",
            r5.tool_calls
        );

        let text5_lower = r5.text.to_lowercase();
        assert!(
            text5_lower.contains("aurora"),
            "Agent must recall the project codename AURORA from memory: {}",
            r5.text
        );
    }
}

// ============================================================================
// SCENARIO 19: Multi-turn context (5 turns testing context window)
// ============================================================================

mod scenario_19_multi_turn_context {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_multi_turn_deep_context() {
        let Some(_api_key) = anthropic_api_key() else {
            eprintln!("Skipping scenario 19: missing ANTHROPIC_API_KEY");
            return;
        };

        let temp_dir = TempDir::new().unwrap();
        let factory = AgentFactory::new(temp_dir.path().join("sessions"));
        let config = Config::default();

        let mut build_config = AgentBuildConfig::new(smoke_model());
        build_config.system_prompt = Some(
            "You are a helpful assistant. Remember all facts the user tells you. Be brief."
                .to_string(),
        );

        let mut agent = factory
            .build_agent(build_config, &config)
            .await
            .expect("Multi-turn context agent should build");

        // Turn 1: Establish fact A
        let r1 = agent
            .run("Fact A: The color of the sky on planet Zorg is green.".to_string())
            .await
            .expect("Turn 1 should succeed");
        eprintln!("[scenario 19] Turn 1: {}", r1.text.trim());

        // Turn 2: Establish fact B
        let r2 = agent
            .run("Fact B: The currency on planet Zorg is called the Glorb.".to_string())
            .await
            .expect("Turn 2 should succeed");
        eprintln!("[scenario 19] Turn 2: {}", r2.text.trim());

        // Turn 3: Establish fact C
        let r3 = agent
            .run("Fact C: The population of planet Zorg is exactly 42 million.".to_string())
            .await
            .expect("Turn 3 should succeed");
        eprintln!("[scenario 19] Turn 3: {}", r3.text.trim());

        // Turn 4: Distractor question
        let r4 = agent
            .run("What is 17 multiplied by 3?".to_string())
            .await
            .expect("Turn 4 should succeed");
        assert!(
            r4.text.contains("51"),
            "Turn 4 should contain 51: {}",
            r4.text
        );
        eprintln!("[scenario 19] Turn 4: {}", r4.text.trim());

        // Turn 5: Recall all three facts
        let r5 = agent
            .run(
                "Please recall all three facts (A, B, and C) about planet Zorg \
                 that I told you earlier."
                    .to_string(),
            )
            .await
            .expect("Turn 5 should succeed");

        let text5_lower = r5.text.to_lowercase();
        eprintln!("[scenario 19] Turn 5: {}", r5.text.trim());

        assert!(
            text5_lower.contains("green"),
            "Should recall the green sky: {}",
            r5.text
        );
        assert!(
            text5_lower.contains("glorb"),
            "Should recall the Glorb currency: {}",
            r5.text
        );
        assert!(
            text5_lower.contains("42") || text5_lower.contains("forty-two"),
            "Should recall population of 42 million: {}",
            r5.text
        );
    }
}

// Scenario 20 removed — was a no-op alias for scenario 8.

// ============================================================================
// SCENARIO 21: SDK Builder — custom profile build + codegen
// ============================================================================

mod scenario_21_sdk_builder {
    use super::*;

    #[tokio::test]
    #[ignore = "integration-real: live API"]
    async fn e2e_smoke_sdk_builder_profile() {
        // This test does NOT require an API key — it tests the build toolchain.
        // However, it requires Python 3 and the build.py script to exist.

        let build_py = {
            let manifest_dir =
                std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set");
            let workspace_root = std::path::Path::new(&manifest_dir)
                .parent()
                .expect("workspace root");
            workspace_root.join("tools/sdk-builder/build.py")
        };

        if !build_py.exists() {
            eprintln!(
                "Skipping scenario 21: build.py not found at {}",
                build_py.display()
            );
            return;
        }

        // Check if python3 is available
        let python_check = tokio::process::Command::new("python3")
            .arg("--version")
            .output()
            .await;
        if python_check.is_err() || !python_check.unwrap().status.success() {
            eprintln!("Skipping scenario 21: python3 not available");
            return;
        }

        // Create a temp directory for the profile and output
        let temp_dir = TempDir::new().unwrap();
        let profile_path = temp_dir.path().join("smoke-test-profile.toml");

        // Write a minimal profile TOML
        let profile_content = r#"
[profile]
name = "smoke-test"

[features]
include = ["comms"]
exclude = ["mcp"]
"#;
        std::fs::write(&profile_path, profile_content).expect("Should write profile TOML");

        // Run the build.py with the profile manifest. The script takes a
        // positional argument (the TOML path). This will attempt a real build
        // which includes: feature resolution, cargo build, schema emission,
        // codegen, and bundle manifest.
        let workspace_root = build_py
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .expect("workspace root");

        let output = tokio::process::Command::new("python3")
            .arg(build_py.to_str().unwrap())
            .arg(profile_path.to_str().unwrap())
            .current_dir(workspace_root)
            .output()
            .await
            .expect("python3 must be available to run build.py");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("[scenario 21] build.py stdout:\n{}", stdout.trim());
        eprintln!("[scenario 21] build.py stderr:\n{}", stderr.trim());

        assert!(
            output.status.success(),
            "build.py must exit 0. status={}, stderr={}",
            output.status,
            stderr.trim()
        );

        // Verify outputs exist
        assert!(
            stdout.contains("Build complete"),
            "build.py should print 'Build complete', got: {}",
            stdout.trim()
        );

        // Verify bundle manifest was written
        let bundle_manifest = workspace_root.join("dist/smoke-test/bundle-manifest.json");
        assert!(
            bundle_manifest.exists(),
            "Bundle manifest should exist at {}",
            bundle_manifest.display()
        );
        let manifest_content =
            std::fs::read_to_string(&bundle_manifest).expect("Should read bundle manifest");
        let manifest: serde_json::Value =
            serde_json::from_str(&manifest_content).expect("Bundle manifest must be valid JSON");
        assert_eq!(
            manifest["profile"], "smoke-test",
            "Bundle manifest profile should be 'smoke-test'"
        );
        eprintln!(
            "[scenario 21] Bundle manifest validated: {}",
            manifest_content.trim()
        );

        // Clean up dist output
        let _ = std::fs::remove_dir_all(workspace_root.join("dist/smoke-test"));
    }
}
