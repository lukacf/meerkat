use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::{
    AgentBuilder, AgentError, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, HookDecision,
    HookEngine, HookExecutionReport, HookId, HookInvocation, HookOutcome, HookPatch, HookPoint,
    HookReasonCode, LlmStreamResult, Message, StopReason, ToolCallView, ToolDef, ToolResult, Usage,
};
use serde_json::Value;
use serde_json::value::RawValue;
use tokio::sync::Mutex;

#[derive(Clone, Copy)]
enum ClientMode {
    TextOnly,
    ToolThenText,
}

struct ScenarioClient {
    mode: ClientMode,
    call_index: AtomicUsize,
    observed_max_tokens: Arc<Mutex<Vec<u32>>>,
}

impl ScenarioClient {
    fn new(mode: ClientMode, observed_max_tokens: Arc<Mutex<Vec<u32>>>) -> Self {
        Self {
            mode,
            call_index: AtomicUsize::new(0),
            observed_max_tokens,
        }
    }
}

#[async_trait]
impl AgentLlmClient for ScenarioClient {
    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&Value>,
    ) -> Result<LlmStreamResult, AgentError> {
        self.observed_max_tokens.lock().await.push(max_tokens);

        let index = self.call_index.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            ClientMode::TextOnly => Ok(LlmStreamResult::new(
                vec![meerkat_core::AssistantBlock::Text {
                    text: "original".to_string(),
                    meta: None,
                }],
                StopReason::EndTurn,
                Usage::default(),
            )),
            ClientMode::ToolThenText => {
                if index == 0 {
                    let args = RawValue::from_string(r#"{"value":"orig"}"#.to_string())
                        .map_err(|e| AgentError::InternalError(e.to_string()))?;
                    Ok(LlmStreamResult::new(
                        vec![meerkat_core::AssistantBlock::ToolUse {
                            id: "tc_1".to_string(),
                            name: "echo".to_string(),
                            args,
                            meta: None,
                        }],
                        StopReason::ToolUse,
                        Usage::default(),
                    ))
                } else {
                    Ok(LlmStreamResult::new(
                        vec![meerkat_core::AssistantBlock::Text {
                            text: "done".to_string(),
                            meta: None,
                        }],
                        StopReason::EndTurn,
                        Usage::default(),
                    ))
                }
            }
        }
    }

    fn provider(&self) -> &'static str {
        "mock"
    }
}

struct RecordingToolDispatcher {
    seen_args: Arc<Mutex<Vec<Value>>>,
}

impl RecordingToolDispatcher {
    fn new(seen_args: Arc<Mutex<Vec<Value>>>) -> Self {
        Self { seen_args }
    }
}

#[async_trait]
impl AgentToolDispatcher for RecordingToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([Arc::new(ToolDef {
            name: "echo".to_string(),
            description: "echo".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        })])
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<ToolResult, meerkat_core::ToolError> {
        let value: Value = serde_json::from_str(call.args.get())
            .map_err(|e| meerkat_core::ToolError::execution_failed(e.to_string()))?;
        self.seen_args.lock().await.push(value.clone());
        Ok(ToolResult::new(
            call.id.to_string(),
            serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
            false,
        ))
    }
}

struct NoopStore;

#[async_trait]
impl AgentSessionStore for NoopStore {
    async fn save(&self, _session: &meerkat_core::Session) -> Result<(), AgentError> {
        Ok(())
    }

    async fn load(&self, _id: &str) -> Result<Option<meerkat_core::Session>, AgentError> {
        Ok(None)
    }
}

#[derive(Default)]
struct TestHookEngine {
    pre_llm_max_tokens: Option<u32>,
    post_llm_text: Option<String>,
    pre_tool_deny: bool,
    pre_tool_args_patch: Option<Value>,
    post_tool_content_patch: Option<String>,
}

#[async_trait]
impl HookEngine for TestHookEngine {
    async fn execute(
        &self,
        invocation: HookInvocation,
        _overrides: Option<&meerkat_core::HookRunOverrides>,
    ) -> Result<HookExecutionReport, meerkat_core::HookEngineError> {
        let mut patches = Vec::new();
        let mut decision = None;

        match invocation.point {
            HookPoint::PreLlmRequest => {
                if let Some(max_tokens) = self.pre_llm_max_tokens {
                    patches.push(HookPatch::LlmRequest {
                        max_tokens: Some(max_tokens),
                        temperature: None,
                        provider_params: None,
                    });
                }
            }
            HookPoint::PostLlmResponse => {
                if let Some(text) = &self.post_llm_text {
                    patches.push(HookPatch::AssistantText { text: text.clone() });
                }
            }
            HookPoint::PreToolExecution => {
                if self.pre_tool_deny {
                    decision = Some(HookDecision::deny(
                        HookId::new("deny-pre-tool"),
                        HookReasonCode::PolicyViolation,
                        "blocked",
                        None,
                    ));
                }
                if let Some(args) = &self.pre_tool_args_patch {
                    patches.push(HookPatch::ToolArgs { args: args.clone() });
                }
            }
            HookPoint::PostToolExecution => {
                if let Some(content) = &self.post_tool_content_patch {
                    patches.push(HookPatch::ToolResult {
                        content: content.clone(),
                        is_error: Some(false),
                    });
                }
            }
            _ => {}
        }

        let outcomes = if patches.is_empty() && decision.is_none() {
            Vec::new()
        } else {
            vec![HookOutcome {
                hook_id: HookId::new("test-hook"),
                point: invocation.point,
                priority: 1,
                registration_index: 0,
                decision: decision.clone(),
                patches: patches.clone(),
                published_patches: vec![],
                error: None,
                duration_ms: Some(0),
            }]
        };

        Ok(HookExecutionReport {
            outcomes,
            decision,
            patches,
            published_patches: vec![],
        })
    }
}

async fn build_agent(
    mode: ClientMode,
    hooks: TestHookEngine,
    seen_args: Arc<Mutex<Vec<Value>>>,
    seen_tokens: Arc<Mutex<Vec<u32>>>,
) -> meerkat_core::Agent<ScenarioClient, RecordingToolDispatcher, NoopStore> {
    let client = Arc::new(ScenarioClient::new(mode, seen_tokens));
    let tools = Arc::new(RecordingToolDispatcher::new(seen_args));
    let store = Arc::new(NoopStore);

    AgentBuilder::new()
        .with_hook_engine(Arc::new(hooks))
        .build(client, tools, store)
        .await
}

#[tokio::test]
async fn pre_tool_deny_blocks_dispatch() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        pre_tool_deny: true,
        ..Default::default()
    };
    let mut agent = build_agent(
        ClientMode::ToolThenText,
        hooks,
        seen_args.clone(),
        seen_tokens,
    )
    .await;

    let _ = agent.run("test".to_string()).await.unwrap();
    assert!(seen_args.lock().await.is_empty());
}

#[tokio::test]
async fn pre_tool_rewrite_mutates_args() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        pre_tool_args_patch: Some(serde_json::json!({"value": "patched"})),
        ..Default::default()
    };
    let mut agent = build_agent(
        ClientMode::ToolThenText,
        hooks,
        seen_args.clone(),
        seen_tokens,
    )
    .await;

    let _ = agent.run("test".to_string()).await.unwrap();
    let args = seen_args.lock().await;
    assert_eq!(args.len(), 1);
    assert_eq!(args[0], serde_json::json!({"value": "patched"}));
}

#[tokio::test]
async fn post_tool_rewrite_mutates_result() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        post_tool_content_patch: Some("patched-result".to_string()),
        ..Default::default()
    };
    let mut agent = build_agent(ClientMode::ToolThenText, hooks, seen_args, seen_tokens).await;

    let _ = agent.run("test".to_string()).await.unwrap();
    let tool_result_message = agent
        .session()
        .messages()
        .iter()
        .find_map(|message| match message {
            Message::ToolResults { results } => Some(results.clone()),
            _ => None,
        })
        .expect("tool results message should exist");

    assert_eq!(tool_result_message[0].content, "patched-result");
}

#[tokio::test]
async fn pre_llm_rewrite_mutates_request_payload() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        pre_llm_max_tokens: Some(42),
        ..Default::default()
    };
    let mut agent = build_agent(ClientMode::TextOnly, hooks, seen_args, seen_tokens.clone()).await;

    let _ = agent.run("test".to_string()).await.unwrap();
    let observed = seen_tokens.lock().await;
    assert_eq!(observed.first().copied(), Some(42));
}

#[tokio::test]
async fn post_llm_rewrite_mutates_assistant_content() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        post_llm_text: Some("patched".to_string()),
        ..Default::default()
    };
    let mut agent = build_agent(ClientMode::TextOnly, hooks, seen_args, seen_tokens).await;

    let result = agent.run("test".to_string()).await.unwrap();
    assert_eq!(result.text, "patched");
}
