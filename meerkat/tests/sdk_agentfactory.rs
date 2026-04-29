#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures::stream;
use meerkat::{
    AgentBuildConfig, AgentBuilder, AgentFactory, AgentLlmClient, AgentToolDispatcher,
    BuildAgentError, Config, CoreAgentBuilder, LlmDoneOutcome, LlmEvent, LlmRequest, ToolDef,
    ToolError, ToolResult,
};
use meerkat_client::LlmClient;
use meerkat_core::ToolDispatchOutcome;
use meerkat_core::{
    AssistantBlock, BlobId, BlobRef, LlmStreamResult, Message, Provider, ProviderImageMetadata,
    RevisedPromptDisposition, StopReason, ToolCallView, Usage,
};
use meerkat_core::{HookEngine, HookEngineError, HookExecutionReport, HookId, HookInvocation};
use meerkat_tools::schema_for;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
#[path = "support/test_session_store.rs"]
mod test_session_store;
use test_session_store::TestSessionStore;

#[allow(dead_code)]
#[derive(Debug, JsonSchema, Deserialize)]
struct EchoInput {
    message: String,
}

struct MockLlmClient {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl LlmClient for MockLlmClient {
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
    > {
        let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
        if call_index == 0 {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: "tc-1".into(),
                    name: "echo".into(),
                    args: json!({"message": "hello"}),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat::StopReason::ToolUse,
                    },
                }),
            ]))
        } else {
            Box::pin(stream::iter(vec![
                Ok(LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success {
                        stop_reason: meerkat::StopReason::EndTurn,
                    },
                }),
            ]))
        }
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

struct RecordingDispatcher {
    called: Arc<AtomicBool>,
}

#[async_trait]
impl AgentToolDispatcher for RecordingDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef {
            name: "echo".into(),
            description: "Echo tool".into(),
            input_schema: schema_for::<EchoInput>(),
            provenance: None,
        })]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.called.store(true, Ordering::SeqCst);
        let args: EchoInput = call
            .parse_args()
            .map_err(|e| ToolError::invalid_arguments(call.name, e.to_string()))?;
        let payload = json!({ "message": args.message, "tool": call.name });
        Ok(ToolResult::new(call.id.to_string(), payload.to_string(), false).into())
    }
}

struct ImageAgentLlmClient;

#[async_trait]
impl AgentLlmClient for ImageAgentLlmClient {
    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, meerkat::AgentError> {
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Image {
                image_id: meerkat_core::AssistantImageId::new(
                    "00000000-0000-4000-8000-000000000124"
                        .parse()
                        .expect("valid uuid"),
                ),
                blob_ref: BlobRef {
                    blob_id: BlobId::new("image-blob-124"),
                    media_type: "image/png".to_string(),
                },
                media_type: meerkat_core::MediaType("image/png".to_string()),
                width: 64,
                height: 32,
                revised_prompt: RevisedPromptDisposition::NotRequested,
                meta: ProviderImageMetadata::NotEmitted,
            }],
            StopReason::EndTurn,
            Usage::default(),
        ))
    }

    fn provider(&self) -> &'static str {
        "anthropic"
    }

    fn model(&self) -> &'static str {
        "claude-sonnet-4-6"
    }
}

struct CustomAgentLlmClient;

#[async_trait]
impl AgentLlmClient for CustomAgentLlmClient {
    async fn stream_response(
        &self,
        _messages: &[Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, meerkat::AgentError> {
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "custom ok".to_string(),
                meta: None,
            }],
            StopReason::EndTurn,
            Usage::default(),
        ))
    }

    fn provider(&self) -> &'static str {
        "custom-provider"
    }

    fn model(&self) -> &'static str {
        "custom-agent-model"
    }
}

struct NoopCompactor;

impl meerkat_core::compact::Compactor for NoopCompactor {
    fn should_compact(&self, _ctx: &meerkat_core::compact::CompactionContext) -> bool {
        false
    }

    fn compaction_prompt(&self) -> &'static str {
        ""
    }

    fn max_summary_tokens(&self) -> u32 {
        1
    }

    fn rebuild_history(
        &self,
        messages: &[Message],
        _summary: &str,
    ) -> meerkat_core::compact::CompactionResult {
        meerkat_core::compact::CompactionResult {
            messages: messages.to_vec(),
            discarded: Vec::new(),
        }
    }
}

struct NoopMemoryStore;

#[async_trait]
impl meerkat_core::memory::MemoryStore for NoopMemoryStore {
    async fn index_scoped_batch(
        &self,
        batch: meerkat_core::memory::MemoryIndexBatch,
    ) -> Result<meerkat_core::memory::MemoryIndexReceipt, meerkat_core::memory::MemoryStoreError>
    {
        Ok(meerkat_core::memory::MemoryIndexReceipt {
            scope: batch.scope().clone(),
            indexed_entries: batch.len(),
        })
    }

    async fn search(
        &self,
        _scope: &meerkat_core::memory::MemorySearchScope,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<meerkat_core::memory::MemoryResult>, meerkat_core::memory::MemoryStoreError>
    {
        Ok(Vec::new())
    }
}

struct ParamsCaptureClient {
    captured: Mutex<serde_json::Value>,
}

impl ParamsCaptureClient {
    fn new() -> Self {
        Self {
            captured: Mutex::new(serde_json::Value::Null),
        }
    }

    fn captured_params(&self) -> Option<serde_json::Value> {
        let value = self.captured.lock().expect("capture mutex").clone();
        if value.is_null() { None } else { Some(value) }
    }
}

#[async_trait]
impl LlmClient for ParamsCaptureClient {
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<
        Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>,
    > {
        *self.captured.lock().expect("capture mutex") = request
            .provider_params
            .as_ref()
            .map(|tag| serde_json::to_value(tag).unwrap_or(serde_json::json!({})))
            .unwrap_or(serde_json::json!({}));
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "ok".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat::StopReason::EndTurn,
                },
            }),
        ]))
    }

    fn provider(&self) -> &'static str {
        "anthropic"
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

struct CountingHookEngine {
    executions: Arc<AtomicUsize>,
}

#[async_trait]
impl HookEngine for CountingHookEngine {
    fn matching_hooks(
        &self,
        _invocation: &HookInvocation,
        _overrides: Option<&meerkat::HookRunOverrides>,
    ) -> Result<Vec<HookId>, HookEngineError> {
        Ok(vec![HookId("counting-hook".to_string())])
    }

    async fn execute(
        &self,
        _invocation: HookInvocation,
        _overrides: Option<&meerkat::HookRunOverrides>,
    ) -> Result<HookExecutionReport, HookEngineError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        Ok(HookExecutionReport::empty())
    }
}

#[tokio::test]
async fn test_sdk_agentfactory_tool_dispatch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let dispatcher_called = Arc::new(AtomicBool::new(false));

    let factory = AgentFactory::new(".rkat/sessions");
    let llm_client = Arc::new(MockLlmClient { calls });
    let llm_adapter = Arc::new(
        factory
            .build_llm_adapter(llm_client, "claude-sonnet-4-6")
            .await,
    );

    let store = Arc::new(TestSessionStore::new());
    let store_adapter = Arc::new(factory.build_store_adapter(store).await);

    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(RecordingDispatcher {
        called: dispatcher_called.clone(),
    });

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-6")
        .max_tokens_per_turn(64)
        .build(llm_adapter, tools, store_adapter)
        .await
        .expect("public builder build");

    let result = agent
        .run("Use the echo tool.".to_string().into())
        .await
        .expect("agent run");

    assert!(
        dispatcher_called.load(Ordering::SeqCst),
        "tool should be dispatched"
    );
    assert!(
        result.text.contains("done"),
        "agent should complete after tool call"
    );
}

#[tokio::test]
async fn public_agentbuilder_matches_factory_session_runtime_invariants() {
    let temp = tempfile::tempdir().expect("tempdir");
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let config = Config::default();
    let model = "claude-sonnet-4-6";
    let prompt = "builder parity prompt";

    let factory_client = Arc::new(MockLlmClient {
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let factory_tools: Arc<dyn AgentToolDispatcher> = Arc::new(RecordingDispatcher {
        called: Arc::new(AtomicBool::new(false)),
    });
    let factory_store = Arc::new(TestSessionStore::new());
    let factory_store = Arc::new(factory.build_store_adapter(factory_store).await);
    let factory_agent = factory
        .build_agent(
            AgentBuildConfig {
                max_tokens: Some(64),
                system_prompt: Some(prompt.to_string()),
                llm_client_override: Some(factory_client),
                tool_dispatcher_override: Some(factory_tools),
                session_store_override: Some(factory_store),
                ..AgentBuildConfig::new(model)
            },
            &config,
        )
        .await
        .expect("factory build");

    let public_client = Arc::new(MockLlmClient {
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let public_llm = Arc::new(factory.build_llm_adapter(public_client, model).await);
    let public_tools: Arc<dyn AgentToolDispatcher> = Arc::new(RecordingDispatcher {
        called: Arc::new(AtomicBool::new(false)),
    });
    let public_store = Arc::new(TestSessionStore::new());
    let public_store = Arc::new(factory.build_store_adapter(public_store).await);

    let public_agent = AgentBuilder::new()
        .model(model)
        .max_tokens_per_turn(64)
        .system_prompt(prompt)
        .build(public_llm, public_tools, public_store)
        .await
        .expect("public builder build");

    let factory_metadata = factory_agent
        .session()
        .session_metadata()
        .expect("factory session metadata");
    let public_metadata = public_agent
        .session()
        .session_metadata()
        .expect("public builder should persist factory session metadata");
    assert_eq!(public_metadata.model, factory_metadata.model);
    assert_eq!(public_metadata.provider, factory_metadata.provider);
    assert_eq!(public_metadata.max_tokens, factory_metadata.max_tokens);
    assert_eq!(
        public_metadata.structured_output_retries,
        factory_metadata.structured_output_retries
    );

    let factory_build_state = factory_agent
        .session()
        .build_state()
        .expect("factory build state");
    let public_build_state = public_agent
        .session()
        .build_state()
        .expect("public builder should persist factory build state");
    assert_eq!(
        public_build_state.system_prompt,
        factory_build_state.system_prompt
    );
    assert_eq!(
        public_build_state.call_timeout_override,
        factory_build_state.call_timeout_override
    );

    assert!(
        factory_agent.execution_snapshot().is_some(),
        "factory path wires runtime turn-state"
    );
    assert!(
        public_agent.execution_snapshot().is_some(),
        "public AgentBuilder must wire the same runtime turn-state default"
    );
}

#[tokio::test]
async fn public_agentbuilder_uses_factory_provider_defaults() {
    let temp = tempfile::tempdir().expect("tempdir");
    let client = Arc::new(ParamsCaptureClient::new());
    let override_client: Arc<dyn LlmClient> = client.clone();
    let mut agent = AgentBuilder::new()
        .with_factory(AgentFactory::new(temp.path().join("sessions")))
        .with_config(Config::default())
        .model("claude-sonnet-4-6")
        .llm_client(override_client)
        .try_build()
        .await
        .expect("public builder build");

    agent.run("test".to_string().into()).await.expect("run");

    let params = client
        .captured_params()
        .expect("factory provider defaults should produce provider params");
    assert!(
        params.get("web_search").is_some(),
        "public AgentBuilder should inherit factory provider defaults: {params}"
    );
}

#[tokio::test]
async fn public_agentbuilder_routes_hook_engine_through_factory() {
    let temp = tempfile::tempdir().expect("tempdir");
    let executions = Arc::new(AtomicUsize::new(0));
    let hook_engine = Arc::new(CountingHookEngine {
        executions: Arc::clone(&executions),
    });
    let client: Arc<dyn LlmClient> = Arc::new(MockLlmClient {
        calls: Arc::new(AtomicUsize::new(1)),
    });

    let mut agent = AgentBuilder::new()
        .with_factory(AgentFactory::new(temp.path().join("sessions")))
        .with_config(Config::default())
        .model("claude-sonnet-4-6")
        .llm_client(client)
        .with_hook_engine(hook_engine)
        .try_build()
        .await
        .expect("public builder build");

    agent.run("test".to_string().into()).await.expect("run");
    assert!(
        executions.load(Ordering::SeqCst) > 0,
        "public AgentBuilder should route hook engine through factory build"
    );
}

#[tokio::test]
async fn public_agentbuilder_with_config_supplies_default_model() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut config = Config::default();
    config.agent.model = "claude-sonnet-4-6".to_string();
    let client: Arc<dyn LlmClient> = Arc::new(MockLlmClient {
        calls: Arc::new(AtomicUsize::new(1)),
    });

    let agent = AgentBuilder::new()
        .with_factory(AgentFactory::new(temp.path().join("sessions")))
        .with_config(config)
        .llm_client(client)
        .try_build()
        .await
        .expect("public builder build");

    let metadata = agent
        .session()
        .session_metadata()
        .expect("factory session metadata");
    assert_eq!(
        metadata.model, "claude-sonnet-4-6",
        "with_config should update the default build model unless .model() was explicit"
    );
}

#[tokio::test]
async fn public_agentbuilder_preserves_agent_llm_image_blocks() {
    let temp = tempfile::tempdir().expect("tempdir");
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let image_client: Arc<dyn AgentLlmClient> = Arc::new(ImageAgentLlmClient);
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(RecordingDispatcher {
        called: Arc::new(AtomicBool::new(false)),
    });
    let store = Arc::new(TestSessionStore::new());
    let store = Arc::new(factory.build_store_adapter(store).await);

    let mut agent = AgentBuilder::new()
        .with_factory(factory)
        .model("claude-sonnet-4-6")
        .build(image_client, tools, store)
        .await
        .expect("public builder build");

    agent.run("return an image".into()).await.expect("run");

    let image_block = agent.session().messages().iter().find_map(|message| {
        if let Message::BlockAssistant(assistant) = message {
            assistant.blocks.iter().find_map(|block| {
                if let AssistantBlock::Image { blob_ref, .. } = block {
                    Some(blob_ref.clone())
                } else {
                    None
                }
            })
        } else {
            None
        }
    });

    assert_eq!(
        image_block
            .expect("image block should be persisted")
            .blob_id,
        BlobId::new("image-blob-124"),
        "public builder compatibility path must preserve AgentLlmClient image blocks"
    );
}

#[tokio::test]
async fn public_agentbuilder_build_uses_agent_llm_provider_for_uncatalogued_model() {
    let temp = tempfile::tempdir().expect("tempdir");
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let custom_client: Arc<dyn AgentLlmClient> = Arc::new(CustomAgentLlmClient);
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(RecordingDispatcher {
        called: Arc::new(AtomicBool::new(false)),
    });
    let store = Arc::new(TestSessionStore::new());
    let store = Arc::new(factory.build_store_adapter(store).await);

    let agent = AgentBuilder::new()
        .with_factory(factory)
        .model("custom-agent-model")
        .build(custom_client, tools, store)
        .await
        .expect("public builder should accept AgentLlmClient provider identity");

    let metadata = agent
        .session()
        .session_metadata()
        .expect("factory metadata");
    assert_eq!(metadata.provider, Provider::Other);
    assert_eq!(metadata.model, "custom-agent-model");
}

#[tokio::test]
async fn public_agentbuilder_build_defaults_to_agent_llm_identity_when_model_is_not_explicit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let custom_client: Arc<dyn AgentLlmClient> = Arc::new(CustomAgentLlmClient);
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(RecordingDispatcher {
        called: Arc::new(AtomicBool::new(false)),
    });
    let store = Arc::new(TestSessionStore::new());
    let store = Arc::new(factory.build_store_adapter(store).await);

    let agent = AgentBuilder::new()
        .with_factory(factory)
        .build(custom_client, tools, store)
        .await
        .expect("public builder should derive default identity from AgentLlmClient");

    let metadata = agent
        .session()
        .session_metadata()
        .expect("factory metadata");
    assert_eq!(metadata.provider, Provider::Other);
    assert_eq!(
        metadata.model, "custom-agent-model",
        "implicit public builder model should not fall back to Config::default() when an AgentLlmClient override supplies identity"
    );
}

fn assert_unsupported_builder_injection(error: BuildAgentError, method: &str) {
    let BuildAgentError::Config(message) = error else {
        panic!("expected Config error for {method}, got {error:?}");
    };
    assert!(
        message.contains(method),
        "error should name unsupported method {method}: {message}"
    );
    assert!(
        message.contains("CoreAgentBuilder"),
        "error should point callers to the standalone builder: {message}"
    );
}

fn unwrap_build_error(
    result: Result<meerkat::DynAgent, BuildAgentError>,
    context: &str,
) -> BuildAgentError {
    match result {
        Ok(_) => panic!("{context}"),
        Err(error) => error,
    }
}

#[tokio::test]
async fn public_agentbuilder_rejects_standalone_core_injections_loudly() {
    let temp = tempfile::tempdir().expect("tempdir");
    let factory = AgentFactory::new(temp.path().join("sessions"));
    let client: Arc<dyn LlmClient> = Arc::new(MockLlmClient {
        calls: Arc::new(AtomicUsize::new(1)),
    });

    let provider_defaults_error = AgentBuilder::new()
        .with_factory(factory.clone())
        .model("claude-sonnet-4-6")
        .llm_client(Arc::clone(&client))
        .provider_tool_defaults(json!({"web_search": {"type": "custom"}}))
        .try_build()
        .await;
    let provider_defaults_error = unwrap_build_error(
        provider_defaults_error,
        "provider_tool_defaults should not be silently ignored",
    );
    assert_unsupported_builder_injection(provider_defaults_error, "provider_tool_defaults");

    let compactor_error = AgentBuilder::new()
        .with_factory(factory.clone())
        .model("claude-sonnet-4-6")
        .llm_client(Arc::clone(&client))
        .compactor(Arc::new(NoopCompactor))
        .try_build()
        .await;
    let compactor_error =
        unwrap_build_error(compactor_error, "compactor should not be silently ignored");
    assert_unsupported_builder_injection(compactor_error, "compactor");

    let memory_error = AgentBuilder::new()
        .with_factory(factory.clone())
        .model("claude-sonnet-4-6")
        .llm_client(Arc::clone(&client))
        .memory_store(Arc::new(NoopMemoryStore))
        .try_build()
        .await;
    let memory_error =
        unwrap_build_error(memory_error, "memory_store should not be silently ignored");
    assert_unsupported_builder_injection(memory_error, "memory_store");

    let turn_state_error = AgentBuilder::new()
        .with_factory(factory)
        .model("claude-sonnet-4-6")
        .llm_client(client)
        .with_turn_state_handle(Arc::new(
            meerkat_runtime::RuntimeTurnStateHandle::ephemeral(),
        ))
        .try_build()
        .await;
    let turn_state_error = unwrap_build_error(
        turn_state_error,
        "with_turn_state_handle should not be silently ignored",
    );
    assert_unsupported_builder_injection(turn_state_error, "with_turn_state_handle");
}

#[tokio::test]
async fn core_agentbuilder_remains_explicit_standalone_escape_hatch() {
    let factory = AgentFactory::new(".rkat/sessions");
    let llm_client = Arc::new(MockLlmClient {
        calls: Arc::new(AtomicUsize::new(1)),
    });
    let llm_adapter = Arc::new(factory.build_llm_adapter(llm_client, "mock-model").await);
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(RecordingDispatcher {
        called: Arc::new(AtomicBool::new(false)),
    });
    let store = Arc::new(TestSessionStore::new());
    let store_adapter = Arc::new(factory.build_store_adapter(store).await);

    let agent = CoreAgentBuilder::new()
        .model("mock-model")
        .max_tokens_per_turn(64)
        .with_turn_state_handle(Arc::new(
            meerkat_runtime::RuntimeTurnStateHandle::ephemeral(),
        ))
        .build(llm_adapter, tools, store_adapter)
        .await;

    assert!(
        agent.session().session_metadata().is_none(),
        "CoreAgentBuilder intentionally remains a standalone primitive builder; \
         facade metadata is owned by meerkat::AgentBuilder/AgentFactory"
    );
}
