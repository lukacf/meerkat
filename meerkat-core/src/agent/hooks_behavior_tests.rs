#![allow(
    clippy::expect_used,
    clippy::field_reassign_with_default,
    clippy::unwrap_used
)]

use crate as meerkat_core;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use meerkat_core::{
    AgentBuilder, AgentError, AgentErrorClass, AgentEvent, AgentLlmClient, AgentSessionStore,
    AgentToolDispatcher, HookDecision, HookEngine, HookExecutionReport, HookId, HookInvocation,
    HookOutcome, HookPatch, HookPoint, HookReasonCode, LlmStreamResult, Message, StopReason,
    ToolCallArguments, ToolCallView, ToolDef, ToolResult, TurnPhase, TurnTerminalOutcome, Usage,
};
use serde_json::Value;
use serde_json::value::RawValue;
use tokio::sync::{Mutex, mpsc};

type DynAgent =
    meerkat_core::Agent<dyn AgentLlmClient, dyn AgentToolDispatcher, dyn AgentSessionStore>;

fn empty_test_tool_schema() -> Value {
    let mut schema = serde_json::Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    Value::Object(schema)
}

#[derive(Clone, Copy)]
enum ClientMode {
    TextOnly,
    ToolThenText,
    ToolThenTextMalformedArgs,
    FailImmediately,
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
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
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
            ClientMode::ToolThenText | ClientMode::ToolThenTextMalformedArgs => {
                if index == 0 {
                    let args_json = match self.mode {
                        ClientMode::ToolThenText => r#"{"value":"orig"}"#.to_string(),
                        ClientMode::ToolThenTextMalformedArgs => {
                            serde_json::to_string("{\"value\":")
                                .map_err(|e| AgentError::InternalError(e.to_string()))?
                        }
                        _ => unreachable!("tool-call branch is only used for tool modes"),
                    };
                    let args = RawValue::from_string(args_json)
                        .map_err(|e| AgentError::InternalError(e.to_string()))?;
                    Ok(LlmStreamResult::new(
                        vec![meerkat_core::AssistantBlock::ToolUse {
                            id: "tc_1".to_string(),
                            name: "echo".into(),
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
            ClientMode::FailImmediately => Err(AgentError::InternalError("llm failed".to_string())),
        }
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    fn model(&self) -> &'static str {
        "mock-model"
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
            name: "echo".into(),
            description: "echo".to_string(),
            input_schema: empty_test_tool_schema(),
            provenance: None,
        })])
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        let value: Value = serde_json::from_str(call.args.get())
            .map_err(|e| meerkat_core::ToolError::execution_failed(e.to_string()))?;
        self.seen_args.lock().await.push(value.clone());
        Ok(ToolResult::new(
            call.id.to_string(),
            serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
            false,
        )
        .into())
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

fn factory_policy_session() -> meerkat_core::Session {
    let mut session = meerkat_core::Session::new();
    session
        .set_session_metadata(meerkat_core::SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "mock-model".to_string(),
            max_tokens: 1024,
            structured_output_retries: 2,
            provider: meerkat_core::Provider::Other,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: meerkat_core::SessionTooling::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
        })
        .expect("test metadata serializes");
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .expect("test build state serializes");
    session
}

#[derive(Default)]
struct TestHookEngine {
    pre_llm_max_tokens: Option<u32>,
    post_llm_text: Option<String>,
    pre_tool_deny: bool,
    run_started_deny: bool,
    turn_boundary_deny: bool,
    pre_tool_args_patch: Option<Value>,
    post_tool_content_patch: Option<String>,
    invocations: Arc<Mutex<Vec<HookPoint>>>,
    pre_tool_seen_args: Arc<Mutex<Vec<Value>>>,
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
        self.invocations.lock().await.push(invocation.point);

        match invocation.point {
            HookPoint::RunStarted if self.run_started_deny => {
                decision = Some(HookDecision::deny(
                    HookId::new("deny-run-started"),
                    HookReasonCode::PolicyViolation,
                    "run start blocked",
                    None,
                ));
            }
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
                if let Some(tool_call) = invocation.tool_call.as_ref() {
                    self.pre_tool_seen_args
                        .lock()
                        .await
                        .push(tool_call.args.as_value().clone());
                }
                if self.pre_tool_deny {
                    decision = Some(HookDecision::deny(
                        HookId::new("deny-pre-tool"),
                        HookReasonCode::PolicyViolation,
                        "blocked",
                        None,
                    ));
                }
                if let Some(args) = &self.pre_tool_args_patch {
                    patches.push(HookPatch::ToolArgs {
                        args: ToolCallArguments::from_value(args.clone())
                            .expect("test hook patch args must be an object"),
                    });
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
            HookPoint::TurnBoundary if self.turn_boundary_deny => {
                decision = Some(HookDecision::deny(
                    HookId::new("deny-turn-boundary"),
                    HookReasonCode::PolicyViolation,
                    "turn boundary blocked",
                    None,
                ));
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
) -> DynAgent {
    let client: Arc<dyn AgentLlmClient> = Arc::new(ScenarioClient::new(mode, seen_tokens));
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(RecordingToolDispatcher::new(seen_args));
    let store: Arc<dyn AgentSessionStore> = Arc::new(NoopStore);

    let builder = AgentBuilder::new()
        .resume_session(factory_policy_session())
        .with_turn_state_handle(Arc::new(
            meerkat_core::agent::test_turn_state_handle::TestTurnStateHandle::new(),
        ))
        .with_hook_engine(Arc::new(hooks));

    builder.build_standalone(client, tools, store).await
}

fn test_hooks() -> TestHookEngine {
    TestHookEngine {
        invocations: Arc::new(Mutex::new(Vec::new())),
        pre_tool_seen_args: Arc::new(Mutex::new(Vec::new())),
        ..Default::default()
    }
}

#[tokio::test]
async fn malformed_provider_tool_call_args_fail_closed_before_event_or_hook_projection() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = test_hooks();
    let invocations = Arc::clone(&hooks.invocations);
    let pre_tool_seen_args = Arc::clone(&hooks.pre_tool_seen_args);
    let mut agent = build_agent(
        ClientMode::ToolThenTextMalformedArgs,
        hooks,
        seen_args.clone(),
        seen_tokens.clone(),
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(32);
    let err = agent
        .run_with_events("test".to_string().into(), tx)
        .await
        .expect_err("malformed provider tool-call args must fail closed");

    assert!(
        matches!(&err, AgentError::ToolError(message)
            if message.contains("Invalid arguments")
                && message.contains("tool call arguments")),
        "unexpected error: {err}"
    );
    assert!(
        seen_args.lock().await.is_empty(),
        "malformed provider args must not reach the dispatcher"
    );
    assert_eq!(
        seen_tokens.lock().await.len(),
        1,
        "malformed provider args must not continue into a follow-up LLM turn"
    );

    let recorded_invocations = invocations.lock().await.clone();
    assert!(
        !recorded_invocations.contains(&HookPoint::PreToolExecution),
        "malformed provider args must not project into PreToolExecution hooks"
    );
    assert!(
        pre_tool_seen_args.lock().await.is_empty(),
        "malformed provider args must not become hook tool-call args"
    );

    let mut saw_run_failed = false;
    let mut saw_tool_call_requested = false;
    let mut saw_tool_result_event = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::RunFailed {
                error_class,
                error_report,
                ..
            } => {
                saw_run_failed = true;
                assert_eq!(error_class, AgentErrorClass::Tool);
                assert!(
                    error_report
                        .as_ref()
                        .is_some_and(|report| report.message.contains("tool call arguments")),
                    "RunFailed should carry the typed projection failure"
                );
            }
            AgentEvent::ToolCallRequested { .. } => saw_tool_call_requested = true,
            AgentEvent::ToolExecutionCompleted { .. } | AgentEvent::ToolResultReceived { .. } => {
                saw_tool_result_event = true;
            }
            _ => {}
        }
    }
    assert!(
        saw_run_failed,
        "malformed provider args should emit RunFailed"
    );
    assert!(
        !saw_tool_call_requested,
        "malformed provider args must not project into ToolCallRequested events"
    );
    assert!(
        !saw_tool_result_event,
        "malformed provider args must not be emitted as a recoverable tool result"
    );
}

#[tokio::test]
async fn valid_provider_tool_call_args_project_to_event_hook_and_dispatch() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = test_hooks();
    let invocations = Arc::clone(&hooks.invocations);
    let pre_tool_seen_args = Arc::clone(&hooks.pre_tool_seen_args);
    let mut agent = build_agent(
        ClientMode::ToolThenText,
        hooks,
        seen_args.clone(),
        seen_tokens.clone(),
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(32);
    agent
        .run_with_events("test".to_string().into(), tx)
        .await
        .expect("valid provider tool-call args should execute");

    assert_eq!(
        seen_args.lock().await.as_slice(),
        [serde_json::json!({"value":"orig"})]
    );
    assert!(
        invocations
            .lock()
            .await
            .contains(&HookPoint::PreToolExecution),
        "valid provider args should project into PreToolExecution hooks"
    );
    assert_eq!(
        pre_tool_seen_args.lock().await.as_slice(),
        [serde_json::json!({"value":"orig"})],
        "valid provider args should project into hook tool-call args"
    );

    let mut saw_tool_call_requested = false;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::ToolCallRequested { args, .. } = event {
            saw_tool_call_requested = true;
            assert_eq!(args.as_value(), &serde_json::json!({"value":"orig"}));
        }
    }
    assert!(
        saw_tool_call_requested,
        "valid provider args should project into ToolCallRequested events"
    );
}

#[tokio::test]
async fn pre_tool_deny_blocks_dispatch() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        pre_tool_deny: true,
        ..test_hooks()
    };
    let mut agent = build_agent(
        ClientMode::ToolThenText,
        hooks,
        seen_args.clone(),
        seen_tokens.clone(),
    )
    .await;

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(32);
    let err = agent
        .run_with_events("test".to_string().into(), tx)
        .await
        .expect_err("PreToolExecution denial should terminalize the run");
    assert!(matches!(
        err,
        AgentError::HookDenied {
            point: HookPoint::PreToolExecution,
            ..
        }
    ));

    assert!(seen_args.lock().await.is_empty());
    assert_eq!(
        seen_tokens.lock().await.len(),
        1,
        "pre-tool denial must not continue into a follow-up LLM turn"
    );
    assert!(
        !agent
            .session()
            .messages()
            .iter()
            .any(|message| matches!(message, Message::ToolResults { .. })),
        "pre-tool denial must not fabricate a transcript ToolResult"
    );

    let snapshot = agent
        .execution_snapshot()
        .expect("test turn-state handle should expose a snapshot");
    assert_eq!(snapshot.turn_phase, TurnPhase::Failed);
    assert_eq!(snapshot.terminal_outcome, TurnTerminalOutcome::Failed);

    let mut saw_run_failed = false;
    let mut saw_tool_result_event = false;
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::RunFailed { .. } => saw_run_failed = true,
            AgentEvent::ToolExecutionCompleted { .. } | AgentEvent::ToolResultReceived { .. } => {
                saw_tool_result_event = true;
            }
            _ => {}
        }
    }
    assert!(saw_run_failed, "pre-tool denial should emit RunFailed");
    assert!(
        !saw_tool_result_event,
        "pre-tool denial should not be emitted as a recoverable tool result"
    );
}

#[tokio::test]
async fn pre_tool_rewrite_mutates_args() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        pre_tool_args_patch: Some(serde_json::json!({"value": "patched"})),
        ..test_hooks()
    };
    let mut agent = build_agent(
        ClientMode::ToolThenText,
        hooks,
        seen_args.clone(),
        seen_tokens,
    )
    .await;

    let _ = agent.run("test".to_string().into()).await.unwrap();
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
        ..test_hooks()
    };
    let mut agent = build_agent(ClientMode::ToolThenText, hooks, seen_args, seen_tokens).await;

    let _ = agent.run("test".to_string().into()).await.unwrap();
    let tool_result_message = agent
        .session()
        .messages()
        .iter()
        .find_map(|message| match message {
            Message::ToolResults { results, .. } => Some(results.clone()),
            _ => None,
        })
        .expect("tool results message should exist");

    assert_eq!(tool_result_message[0].text_content(), "patched-result");
}

#[tokio::test]
async fn pre_llm_rewrite_mutates_request_payload() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        pre_llm_max_tokens: Some(42),
        ..test_hooks()
    };
    let mut agent = build_agent(ClientMode::TextOnly, hooks, seen_args, seen_tokens.clone()).await;

    let _ = agent.run("test".to_string().into()).await.unwrap();
    let observed = seen_tokens.lock().await;
    assert_eq!(observed.first().copied(), Some(42));
}

#[tokio::test]
async fn post_llm_rewrite_mutates_assistant_content() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        post_llm_text: Some("patched".to_string()),
        ..test_hooks()
    };
    let mut agent = build_agent(ClientMode::TextOnly, hooks, seen_args, seen_tokens).await;

    let result = agent.run("test".to_string().into()).await.unwrap();
    assert_eq!(result.text, "patched");
}

#[tokio::test]
async fn run_started_deny_blocks_run() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        run_started_deny: true,
        ..test_hooks()
    };
    let mut agent = build_agent(ClientMode::TextOnly, hooks, seen_args, seen_tokens).await;

    let err = agent.run("test".to_string().into()).await.unwrap_err();
    assert!(matches!(
        err,
        AgentError::HookDenied {
            point: HookPoint::RunStarted,
            ..
        }
    ));
}

#[tokio::test]
async fn turn_boundary_deny_blocks_next_turn() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = TestHookEngine {
        turn_boundary_deny: true,
        ..test_hooks()
    };
    let mut agent = build_agent(ClientMode::ToolThenText, hooks, seen_args, seen_tokens).await;

    let err = agent.run("test".to_string().into()).await.unwrap_err();
    assert!(matches!(
        err,
        AgentError::HookDenied {
            point: HookPoint::TurnBoundary,
            ..
        }
    ));
}

#[tokio::test]
async fn run_failed_hook_is_invoked_on_llm_error() {
    let seen_args = Arc::new(Mutex::new(Vec::new()));
    let seen_tokens = Arc::new(Mutex::new(Vec::new()));
    let hooks = test_hooks();
    let invocations = hooks.invocations.clone();
    let mut agent = build_agent(ClientMode::FailImmediately, hooks, seen_args, seen_tokens).await;

    let _ = agent.run("test".to_string().into()).await.unwrap_err();
    let seen = invocations.lock().await.clone();
    assert!(seen.contains(&HookPoint::RunFailed));
}
