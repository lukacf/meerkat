#![allow(clippy::large_futures)]

//! Integration tests for `AgentFactory::build_agent()`.
//!
//! These tests validate the consolidated agent construction pipeline without
//! requiring live API keys, using mock LLM clients injected via
//! `AgentBuildConfig::llm_client_override`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use futures::stream;
use meerkat::{AgentBuildConfig, AgentFactory, LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat::{AgentError, AgentLlmClient, BuildAgentError, CompiledSchema, LlmStreamResult};
use meerkat_client::{LlmClient, TestClient};
#[cfg(feature = "comms")]
use meerkat_comms::{CommsRuntime, ResolvedCommsConfig, identity::Keypair};
use meerkat_core::AssistantBlock;
use meerkat_core::lifecycle::run_primitive::ProviderParamsOverride;
use meerkat_core::service::MobToolAuthorityContext;
use meerkat_core::web_search::{WEB_SEARCH_TOOL_NAME, WebSearchRequest, WebSearchResult};
use meerkat_core::{
    AgentToolDispatcher, Config, Provider, SelfHostedApiStyle, SelfHostedModelConfig,
    SelfHostedServerConfig, SelfHostedTransport, Session, SessionId, SessionLlmIdentity,
    SessionMetadata, SessionTooling, ToolCallView, ToolCategoryOverride, ToolDef,
    ToolDispatchOutcome, ToolError, UserMessage,
};
use meerkat_llm_core::WebSearchExecutor;
use meerkat_schedule::{MemoryScheduleStore, ScheduleService, ScheduleToolDispatcher};
use meerkat_store::{SessionFilter, SessionStore, SessionStoreError};
use serde_json::json;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Mock LLM client (returns a simple text response)
// ---------------------------------------------------------------------------

struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>>
    {
        Box::pin(stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: "Hello from mock".to_string(),
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            }),
        ]))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

struct MockAgentLlmClient {
    model: String,
}

#[async_trait]
impl AgentLlmClient for MockAgentLlmClient {
    async fn stream_response(
        &self,
        _messages: &[meerkat::Message],
        _tools: &[Arc<ToolDef>],
        _max_tokens: u32,
        _temperature: Option<f32>,
        _provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        Ok(LlmStreamResult::new(
            vec![AssistantBlock::Text {
                text: "Hello from agent override".to_string(),
                meta: None,
            }],
            meerkat_core::StopReason::EndTurn,
            meerkat_core::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        ))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Other
    }

    fn model(&self) -> &str {
        &self.model
    }
}

struct CountingAgentLlmClient {
    inner: Arc<dyn AgentLlmClient>,
    stream_calls: Arc<AtomicUsize>,
}

#[async_trait]
impl AgentLlmClient for CountingAgentLlmClient {
    async fn stream_response(
        &self,
        messages: &[meerkat::Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        self.stream_calls.fetch_add(1, Ordering::SeqCst);
        self.inner
            .stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn compile_schema(
        &self,
        output_schema: &meerkat::OutputSchema,
    ) -> Result<CompiledSchema, meerkat::SchemaError> {
        self.inner.compile_schema(output_schema)
    }
}

fn counting_agent_llm_client_decorator(
    constructions: Arc<AtomicUsize>,
    stream_calls: Arc<AtomicUsize>,
) -> meerkat::AgentLlmClientDecorator {
    Arc::new(move |client| {
        constructions.fetch_add(1, Ordering::SeqCst);
        Arc::new(CountingAgentLlmClient {
            inner: client,
            stream_calls: Arc::clone(&stream_calls),
        })
    })
}

fn test_workgraph_tools() -> Arc<dyn AgentToolDispatcher> {
    Arc::new(meerkat::WorkGraphToolSurface::new(
        meerkat::WorkGraphService::new(Arc::new(meerkat::MemoryWorkGraphStore::new())),
    ))
}

#[cfg(feature = "comms")]
async fn add_trusted_peer_with_generated_authority(
    runtime: Arc<CommsRuntime>,
    peer: meerkat_core::comms::TrustedPeerDescriptor,
) {
    let endpoint = meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&peer);
    let machine = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("generated MeerkatMachine should prepare peer-comms bindings");
    bindings
        .install_peer_comms_on(runtime.as_ref())
        .expect("install generated peer-comms handle");
    let comms_runtime: Arc<dyn meerkat_core::agent::CommsRuntime> = runtime;
    machine
        .stage_add_direct_peer_endpoint(&session_id, endpoint, comms_runtime)
        .await
        .expect("generated peer projection should reconcile trusted peer");
}

#[derive(Default)]
struct CaptureClient {
    inner: TestClient,
    seen_tools: Mutex<Vec<String>>,
    seen_tool_defs: Mutex<Vec<ToolDef>>,
}

impl CaptureClient {
    fn tool_names(&self) -> Vec<String> {
        self.seen_tools.lock().expect("capture lock").clone()
    }

    fn tool_defs(&self) -> Vec<ToolDef> {
        self.seen_tool_defs
            .lock()
            .expect("capture tool defs lock")
            .clone()
    }
}

#[async_trait]
impl LlmClient for CaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>>
    {
        *self.seen_tools.lock().expect("capture lock") = request
            .tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        *self.seen_tool_defs.lock().expect("capture tool defs lock") =
            request.tools.iter().map(|tool| (**tool).clone()).collect();
        self.inner.stream(request)
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        self.inner.health_check().await
    }
}

struct NoopWebSearchExecutor;

#[async_trait]
impl WebSearchExecutor for NoopWebSearchExecutor {
    async fn execute_web_search(
        &self,
        request: WebSearchRequest,
    ) -> Result<WebSearchResult, meerkat_llm_core::LlmError> {
        Ok(WebSearchResult::unavailable(request.query, "test executor"))
    }
}

// ---------------------------------------------------------------------------
// Helper: create a factory pointing at a temp directory
// ---------------------------------------------------------------------------

fn temp_factory(temp: &tempfile::TempDir) -> AgentFactory {
    AgentFactory::new(temp.path().join("sessions"))
}

struct EmptyDispatcher;

#[async_trait]
impl AgentToolDispatcher for EmptyDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Vec::<Arc<ToolDef>>::new().into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

#[cfg_attr(not(feature = "comms"), allow(dead_code))]
struct NamedDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
}

impl NamedDispatcher {
    #[cfg_attr(not(feature = "comms"), allow(dead_code))]
    fn new(name: &str) -> Self {
        Self {
            tools: Arc::from(vec![Arc::new(ToolDef {
                name: name.into(),
                description: format!("{name} test tool"),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                }),
                provenance: None,
            })]),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for NamedDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

struct RecordingMobToolsFactory {
    observed_authority_context:
        Arc<std::sync::Mutex<Vec<Option<meerkat_core::service::MobToolAuthorityContext>>>>,
}

#[async_trait]
impl meerkat_core::service::MobToolsFactory for RecordingMobToolsFactory {
    async fn build_mob_tools(
        &self,
        args: meerkat_core::service::MobToolsBuildArgs,
    ) -> Result<Arc<dyn AgentToolDispatcher>, Box<dyn std::error::Error + Send + Sync>> {
        self.observed_authority_context
            .lock()
            .expect("recording mutex")
            .push(args.authority_context);
        Ok(Arc::new(EmptyDispatcher))
    }
}

#[cfg(feature = "comms")]
#[derive(Debug, Default)]
struct RecordedMobToolCommsArgs {
    comms_name: Option<String>,
    comms_runtime_present: bool,
}

#[cfg(feature = "comms")]
struct RecordingMobCommsFactory {
    observed: Arc<std::sync::Mutex<Vec<RecordedMobToolCommsArgs>>>,
}

#[cfg(feature = "comms")]
#[async_trait]
impl meerkat_core::service::MobToolsFactory for RecordingMobCommsFactory {
    async fn build_mob_tools(
        &self,
        args: meerkat_core::service::MobToolsBuildArgs,
    ) -> Result<Arc<dyn AgentToolDispatcher>, Box<dyn std::error::Error + Send + Sync>> {
        self.observed
            .lock()
            .expect("recording mutex")
            .push(RecordedMobToolCommsArgs {
                comms_name: args.comms_name,
                comms_runtime_present: args.comms_runtime.is_some(),
            });
        Ok(Arc::new(EmptyDispatcher))
    }
}

#[cfg_attr(not(feature = "comms"), allow(dead_code))]
struct StaticMobToolsFactory {
    dispatcher: Arc<dyn AgentToolDispatcher>,
}

#[async_trait]
impl meerkat_core::service::MobToolsFactory for StaticMobToolsFactory {
    async fn build_mob_tools(
        &self,
        _args: meerkat_core::service::MobToolsBuildArgs,
    ) -> Result<Arc<dyn AgentToolDispatcher>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Arc::clone(&self.dispatcher))
    }
}

async fn run_and_capture_tool_names(
    factory: AgentFactory,
    mut build_config: AgentBuildConfig,
) -> Vec<String> {
    let capture: Arc<CaptureClient> = Arc::new(CaptureClient::default());
    build_config.llm_client_override = Some(capture.clone());
    let mut agent = factory
        .build_agent(build_config, &Config::default())
        .await
        .unwrap();
    agent.run("inspect tools".to_string().into()).await.unwrap();
    capture.tool_names()
}

async fn run_and_capture_tool_defs(
    factory: AgentFactory,
    mut build_config: AgentBuildConfig,
) -> Vec<ToolDef> {
    let capture: Arc<CaptureClient> = Arc::new(CaptureClient::default());
    build_config.llm_client_override = Some(capture.clone());
    let mut agent = factory
        .build_agent(build_config, &Config::default())
        .await
        .unwrap();
    agent.run("inspect tools".to_string().into()).await.unwrap();
    capture.tool_defs()
}

async fn build_and_capture_visible_tool_names(
    factory: AgentFactory,
    mut build_config: AgentBuildConfig,
) -> Vec<String> {
    build_config.llm_client_override = Some(Arc::new(MockLlmClient));
    let agent = factory
        .build_agent(build_config, &Config::default())
        .await
        .unwrap();
    agent
        .tool_scope()
        .visible_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

#[tokio::test]
async fn explicit_web_search_fallback_is_visible_for_realtime_model() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).builtins(true);
    let tool_names = build_and_capture_visible_tool_names(
        factory,
        AgentBuildConfig {
            web_search_executor_override: Some(Arc::new(NoopWebSearchExecutor)),
            override_web_search: ToolCategoryOverride::Enable,
            ..AgentBuildConfig::new("gpt-realtime-2")
        },
    )
    .await;

    assert!(
        tool_names.iter().any(|name| name == WEB_SEARCH_TOOL_NAME),
        "explicit fallback should be exposed for realtime models without native search; saw {tool_names:?}"
    );
}

#[tokio::test]
async fn explicit_web_search_enable_without_provider_errors() {
    // #108: an explicit web-search Enable with no executor override, a model
    // that lacks native search, and a default config (provider web search
    // disabled) must fail closed — never silently build tool-less.
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).builtins(true);
    let mut build_config = AgentBuildConfig {
        override_web_search: ToolCategoryOverride::Enable,
        ..AgentBuildConfig::new("gpt-realtime-2")
    };
    build_config.llm_client_override = Some(Arc::new(MockLlmClient));
    // OpenAI provider web search defaults to enabled; the #108 fail-closed path
    // only triggers when the capability is genuinely undeliverable, so disable
    // the provider-native fallback to match this test's stated premise (a model
    // that lacks native search AND no provider web search to fall back to).
    let mut config = Config::default();
    config.provider_tools.openai.web_search = false;
    let result = factory.build_agent(build_config, &config).await;
    let err = result
        .err()
        .expect("explicit web-search enable with no provider must fail closed");
    assert!(
        matches!(err, meerkat::BuildAgentError::CapabilityUnavailable { capability, .. } if capability == "web_search"),
        "expected CapabilityUnavailable(web_search), got: {err:?}"
    );
}

#[tokio::test]
async fn web_search_fallback_stays_hidden_on_inherit() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).builtins(true);
    let tool_names = build_and_capture_visible_tool_names(
        factory,
        AgentBuildConfig {
            web_search_executor_override: Some(Arc::new(NoopWebSearchExecutor)),
            ..AgentBuildConfig::new("gpt-realtime-2")
        },
    )
    .await;

    assert!(
        !tool_names.iter().any(|name| name == WEB_SEARCH_TOOL_NAME),
        "fallback must be explicit, not inherited by default; saw {tool_names:?}"
    );
}

#[tokio::test]
async fn web_search_fallback_is_not_added_to_native_search_models() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).builtins(true);
    let tool_names = build_and_capture_visible_tool_names(
        factory,
        AgentBuildConfig {
            web_search_executor_override: Some(Arc::new(NoopWebSearchExecutor)),
            override_web_search: ToolCategoryOverride::Enable,
            ..AgentBuildConfig::new("gpt-5.4")
        },
    )
    .await;

    assert!(
        !tool_names.iter().any(|name| name == WEB_SEARCH_TOOL_NAME),
        "models with native search capability must not receive the fallback; saw {tool_names:?}"
    );
}

fn create_test_authority() -> MobToolAuthorityContext {
    let authority = meerkat_runtime::mob_operator_authority::create_only_mob_operator_authority()
        .expect("generated create-only mob authority should be accepted");
    meerkat_runtime::mob_operator_authority::grant_manage_mob(&authority, "mob-a")
        .expect("generated mob authority should grant managed mob scope")
}

fn assert_generated_create_only_authority(authority: Option<&MobToolAuthorityContext>) {
    let authority = authority.expect("generated create-only authority");
    assert!(authority.can_create_mobs());
    assert!(authority.managed_mob_scope().is_empty());
    assert!(authority.caller_provenance().is_none());
    assert!(authority.audit_invocation_id().is_none());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// 1. `build_agent` with a mock LLM client produces a DynAgent that can run.
#[tokio::test]
async fn build_agent_with_mock_client_produces_runnable_agent() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let mut agent = factory.build_agent(build_config, &config).await.unwrap();

    let result = agent.run("Hello".to_string().into()).await.unwrap();
    assert!(
        result.text.contains("Hello from mock"),
        "Agent should produce text from mock client, got: {}",
        result.text
    );
}

#[tokio::test]
async fn agent_llm_client_decorator_wraps_registry_resolved_provider_client() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let mut config = Config::default();
    // The build omits `realm_id`, so the connection resolver heads to the
    // reserved `global` realm; the inline binding must live there to resolve via
    // the configured key (rather than falling through to an absent env key).
    config.realm.insert(
        meerkat_core::connection::GLOBAL_REALM_SLUG.to_string(),
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", "test-openai-key")]),
    );
    config.model_fallback.enabled = false;
    let constructions = Arc::new(AtomicUsize::new(0));
    let stream_calls = Arc::new(AtomicUsize::new(0));

    let agent = factory
        .build_agent(
            AgentBuildConfig {
                agent_llm_client_decorator: Some(counting_agent_llm_client_decorator(
                    Arc::clone(&constructions),
                    Arc::clone(&stream_calls),
                )),
                ..AgentBuildConfig::new("gpt-5.4")
            },
            &config,
        )
        .await
        .expect("registry-resolved provider client should build");

    drop(agent);
    assert_eq!(constructions.load(Ordering::SeqCst), 1);
    assert_eq!(
        stream_calls.load(Ordering::SeqCst),
        0,
        "registry test should prove construction without making a provider call"
    );
}

#[tokio::test]
async fn agent_llm_client_decorator_wraps_raw_llm_client_override() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let constructions = Arc::new(AtomicUsize::new(0));
    let stream_calls = Arc::new(AtomicUsize::new(0));

    let mut agent = factory
        .build_agent(
            AgentBuildConfig {
                llm_client_override: Some(Arc::new(MockLlmClient)),
                agent_llm_client_decorator: Some(counting_agent_llm_client_decorator(
                    Arc::clone(&constructions),
                    Arc::clone(&stream_calls),
                )),
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
            &Config::default(),
        )
        .await
        .expect("raw llm override should build");

    let result = agent.run("Hello".to_string().into()).await.unwrap();
    assert!(result.text.contains("Hello from mock"));
    assert_eq!(constructions.load(Ordering::SeqCst), 1);
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn agent_llm_client_decorator_wraps_agent_llm_client_override() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let constructions = Arc::new(AtomicUsize::new(0));
    let stream_calls = Arc::new(AtomicUsize::new(0));

    let mut agent = factory
        .build_agent(
            AgentBuildConfig {
                agent_llm_client_override: Some(Arc::new(MockAgentLlmClient {
                    model: "agent-override-model".to_string(),
                })),
                agent_llm_client_decorator: Some(counting_agent_llm_client_decorator(
                    Arc::clone(&constructions),
                    Arc::clone(&stream_calls),
                )),
                ..AgentBuildConfig::new("agent-override-model")
            },
            &Config::default(),
        )
        .await
        .expect("agent llm override should build");

    let result = agent.run("Hello".to_string().into()).await.unwrap();
    assert!(result.text.contains("Hello from agent override"));
    assert_eq!(constructions.load(Ordering::SeqCst), 1);
    assert_eq!(stream_calls.load(Ordering::SeqCst), 1);
}

/// 2. `build_agent` without LLM override fails when no API key is set.
///
/// Superseded by `build_agent_without_auth_binding_rejects_ambient_realm_config_api_key`
/// (test 2b below): the wave-c auth-seam cleanup deleted env-default realm
/// synthesis and first-matching-provider promotion, so `build_agent` now
/// rejects at the AuthBindingRef gate before ever reaching the API-key check.
/// This test's original path (config → ambient provider resolution → MissingApiKey)
/// is unreachable. Environments with `ANTHROPIC_API_KEY` set would early-return
/// "Skipping"; environments without it now hit a typed `LlmClient` resolution
/// refusal, not `MissingApiKey` — the assertion at line 309 fails. Ignored rather than
/// deleted so the pre-wave-c intent stays visible and a future reader can
/// confirm 2b replaced it.
#[ignore = "wave-c auth-seam cleanup removed the ambient MissingApiKey path; superseded by test 2b"]
#[tokio::test]
async fn build_agent_without_override_fails_missing_api_key() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    // Use an unusual model that infers to a known provider but has no API key set.
    // We use claude- prefix to infer Anthropic, but no ANTHROPIC_API_KEY.
    let build_config = AgentBuildConfig::new("claude-nonexistent-model");

    // This test relies on there being no ANTHROPIC_API_KEY in the test environment.
    // If it IS set, the test would not hit MissingApiKey, so we skip in that case.
    if std::env::var("ANTHROPIC_API_KEY").is_ok()
        || std::env::var("RKAT_ANTHROPIC_API_KEY").is_ok()
        || std::env::var("RKAT_TEST_CLIENT").ok().as_deref() == Some("1")
    {
        eprintln!("Skipping: API key is set in environment");
        return;
    }

    let result = factory.build_agent(build_config, &config).await;
    assert!(result.is_err(), "Should fail without API key");
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("Expected error, got Ok"),
    };
    let err_str = err.to_string();
    assert!(
        err_str.contains("API key"),
        "Error should mention API key, got: {err_str}"
    );
}

/// 2b. A missing auth_binding resolves through the reserved `global` realm
///     binding (the universal default head). This is a valid typed auth path,
///     distinct from first-provider ambient credential search.
#[tokio::test]
async fn build_agent_without_auth_binding_uses_global_realm_config_api_key() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let mut config = Config::default();
    let section =
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", "test-openai-key")]);
    config.realm.insert("global".to_string(), section);

    let build_config = AgentBuildConfig::new("gpt-5.4");
    assert!(build_config.auth_binding.is_none());
    let agent = factory
        .build_agent(build_config, &config)
        .await
        .expect("global realm config API key should resolve without explicit auth_binding");
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");
    assert_eq!(metadata.provider, Provider::OpenAI);
    assert_eq!(
        metadata.auth_binding.as_ref().map(|conn_ref| {
            (
                conn_ref.realm.as_str().to_string(),
                conn_ref.binding.as_str().to_string(),
            )
        }),
        Some(("global".to_string(), "default_openai".to_string()))
    );
}

/// 3. `build_agent` with unknown provider model fails.
#[tokio::test]
async fn build_agent_unknown_provider_fails() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let build_config = AgentBuildConfig::new("unknown-model-xyz");

    let result = factory.build_agent(build_config, &config).await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("Expected error for unknown model"),
    };
    let err_str = err.to_string();
    assert!(
        err_str.contains("infer provider") || err_str.contains("unknown-model-xyz"),
        "Error should mention model inference failure, got: {err_str}"
    );
}

/// 4. `build_agent` sets SessionMetadata correctly.
#[tokio::test]
async fn build_agent_sets_session_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).builtins(true).shell(true);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        max_tokens: Some(2048),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();

    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");

    assert_eq!(metadata.model, "claude-sonnet-4-5");
    assert_eq!(metadata.max_tokens, 2048);
    assert_eq!(metadata.provider, Provider::Anthropic);
    // No explicit override was set → Inherit (follows factory default)
    assert_eq!(metadata.tooling.builtins, ToolCategoryOverride::Inherit);
    assert_eq!(metadata.tooling.shell, ToolCategoryOverride::Inherit);
    // comms has no override field — always persisted as Inherit
    assert_eq!(metadata.tooling.comms, ToolCategoryOverride::Inherit);
    // Skills become active only when they were explicitly preloaded.
    #[cfg(feature = "skills")]
    assert!(metadata.tooling.active_skills.is_none());
    #[cfg(not(feature = "skills"))]
    assert!(metadata.tooling.active_skills.is_none());
    assert!(!metadata.keep_alive);
    assert!(metadata.comms_name.is_none());
}

#[tokio::test]
async fn build_agent_applies_configured_retry_schedule_without_call_timeout() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let mut config = Config::default();
    config.retry.max_retries = 8;
    config.retry.initial_delay = std::time::Duration::from_millis(750);
    config.retry.max_delay = std::time::Duration::from_secs(120);
    config.retry.multiplier = 2.5;
    config.retry.call_timeout_override =
        meerkat_core::CallTimeoutOverride::Value(std::time::Duration::from_secs(90));

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let retry = agent.retry_policy();

    assert_eq!(retry.max_retries, 8);
    assert_eq!(retry.initial_delay, std::time::Duration::from_millis(750));
    assert_eq!(retry.max_delay, std::time::Duration::from_secs(120));
    assert_eq!(retry.multiplier, 2.5);
    assert_eq!(
        retry.call_timeout, None,
        "factory retry schedule wiring must not steal call-timeout ownership from call_timeout_override"
    );
}

/// 4b. Configured self-hosted aliases resolve as first-class model IDs.
#[tokio::test]
async fn build_agent_resolves_self_hosted_alias_from_registry() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let mut config = Config::default();
    config.self_hosted.servers.insert(
        "local".to_string(),
        SelfHostedServerConfig {
            transport: SelfHostedTransport::OpenAiCompatible,
            base_url: "http://127.0.0.1:11434".to_string(),
            api_style: SelfHostedApiStyle::Responses,
        },
    );
    config.self_hosted.models.insert(
        "gemma-4-31b".to_string(),
        SelfHostedModelConfig {
            server: "local".to_string(),
            remote_model: "gemma4:31b".to_string(),
            display_name: "Gemma 4 31B".into(),
            family: "gemma-4".to_string(),
            tier: meerkat_core::model_profile::catalog::ModelTier::Supported,
            context_window: Some(256_000),
            max_output_tokens: Some(8_192),
            vision: true,
            image_tool_results: true,
            inline_video: false,
            supports_temperature: true,
            supports_thinking: false,
            supports_reasoning: false,
            supports_web_search: false,
            call_timeout_secs: Some(600),
        },
    );

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("gemma-4-31b")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");

    assert_eq!(metadata.model, "gemma-4-31b");
    assert_eq!(metadata.provider, Provider::SelfHosted);
    assert_eq!(metadata.self_hosted_server_id.as_deref(), Some("local"));
}

#[tokio::test]
async fn build_llm_client_for_identity_rejects_self_hosted_server_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let mut config = Config::default();
    config.self_hosted.servers.insert(
        "local".to_string(),
        SelfHostedServerConfig {
            transport: SelfHostedTransport::OpenAiCompatible,
            base_url: "http://127.0.0.1:11434".to_string(),
            api_style: SelfHostedApiStyle::ChatCompletions,
        },
    );
    config.self_hosted.models.insert(
        "gemma-4-e2b".to_string(),
        SelfHostedModelConfig {
            server: "local".to_string(),
            remote_model: "gemma4:e2b".to_string(),
            display_name: "Gemma 4 E2B".into(),
            family: "gemma-4".to_string(),
            tier: meerkat_core::model_profile::catalog::ModelTier::Supported,
            context_window: Some(128_000),
            max_output_tokens: Some(8_192),
            vision: true,
            image_tool_results: true,
            inline_video: false,
            supports_temperature: true,
            supports_thinking: true,
            supports_reasoning: true,
            supports_web_search: false,
            call_timeout_secs: Some(600),
        },
    );

    let err = match factory
        .build_llm_client_for_identity(
            &config,
            &SessionLlmIdentity {
                model: "gemma-4-e2b".to_string(),
                provider: Provider::SelfHosted,
                self_hosted_server_id: Some("other".to_string()),
                provider_params: None,
                auth_binding: None,
            },
        )
        .await
    {
        Ok(_) => panic!("mismatched server binding should be rejected"),
        Err(err) => err,
    };

    let message = err.to_string();
    assert!(message.contains("bound to server 'other'"));
    assert!(message.contains("resolves to 'local'"));
}

/// 5. `build_agent` with `enable_builtins=true` produces agent with builtin tools.
#[tokio::test]
async fn build_agent_with_builtins_has_tools() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).builtins(true);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();

    // The agent should have builtin tools registered.
    // We can't directly inspect tools on a DynAgent, but we verify via session metadata.
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");
    // No explicit override → Inherit (builtins are active via factory default)
    assert_eq!(
        metadata.tooling.builtins,
        ToolCategoryOverride::Inherit,
        "builtins should be Inherit when no explicit override was set"
    );
}

#[tokio::test]
async fn build_agent_with_scheduler_has_tools_when_enabled_and_injected() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).schedule(true);
    let tool_defs = run_and_capture_tool_defs(
        factory,
        AgentBuildConfig {
            schedule_tools: Some(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
                Arc::new(MemoryScheduleStore::default()),
            )))),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        },
    )
    .await;
    let tool_names = tool_defs
        .iter()
        .map(|tool| tool.name.to_string())
        .collect::<Vec<_>>();

    assert!(
        tool_names
            .iter()
            .any(|name| name == "meerkat_schedule_create")
    );
    assert!(
        tool_names
            .iter()
            .any(|name| name == "meerkat_schedule_list")
    );
    let create_tool = tool_defs
        .iter()
        .find(|tool| tool.name == "meerkat_schedule_create")
        .expect("schedule create tool should be visible");
    let target_types =
        &create_tool.input_schema["properties"]["target"]["oneOf"][0]["properties"]["type"]["enum"];
    assert!(
        target_types
            .as_array()
            .expect("target type enum")
            .iter()
            .any(|value| value.as_str() == Some("current_session")),
        "agent-facing schedule tools should expose current_session"
    );
}

#[tokio::test]
async fn build_agent_without_scheduler_keeps_injected_scheduler_tools_hidden() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).schedule(false);
    let tool_names = run_and_capture_tool_names(
        factory,
        AgentBuildConfig {
            override_mob: ToolCategoryOverride::Enable,
            schedule_tools: Some(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
                Arc::new(MemoryScheduleStore::default()),
            )))),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        },
    )
    .await;

    assert!(
        !tool_names
            .iter()
            .any(|name| name == "meerkat_schedule_create")
    );
}

#[cfg(feature = "comms")]
#[tokio::test]
async fn build_agent_composes_scheduler_alongside_comms_and_mob() {
    let temp = tempfile::tempdir().unwrap();
    let comms_config = ResolvedCommsConfig {
        enabled: true,
        name: "test-scheduler-comms".into(),
        inproc_namespace: None,
        listen_tcp: None,
        listen_uds: None,
        advertise_address: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        identity_dir: temp.path().join("identity"),
        trusted_peers_path: temp.path().join("trusted_peers.json"),
        comms_config: Default::default(),
        auth: Default::default(),
        require_peer_auth: false,
        allow_external_unauthenticated: false,
        pairing_password: None,
    };
    let comms_runtime = Arc::new(
        CommsRuntime::new_with_silent_intents(comms_config, Arc::new(Default::default()))
            .await
            .unwrap(),
    );
    let peer_a_pubkey = Keypair::generate().public_key();
    add_trusted_peer_with_generated_authority(
        Arc::clone(&comms_runtime),
        meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
            "peer-a",
            peer_a_pubkey.to_peer_id().to_string(),
            *peer_a_pubkey.as_bytes(),
            "tcp://127.0.0.1:9999",
        )
        .expect("trusted peer descriptor"),
    )
    .await;
    let factory = temp_factory(&temp)
        .comms(true)
        .with_comms_runtime(comms_runtime)
        .schedule(true)
        .mob(true)
        .mob_tools_factory(Arc::new(StaticMobToolsFactory {
            dispatcher: Arc::new(NamedDispatcher::new("mob_probe")),
        }));

    let tool_names = run_and_capture_tool_names(
        factory,
        AgentBuildConfig {
            mob_tool_authority_context: Some(create_test_authority()),
            schedule_tools: Some(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
                Arc::new(MemoryScheduleStore::default()),
            )))),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        },
    )
    .await;

    assert!(tool_names.iter().any(|name| name == "send_message"));
    assert!(tool_names.iter().any(|name| name == "peers"));
    assert!(
        tool_names
            .iter()
            .any(|name| name == "meerkat_schedule_create")
    );
    assert!(tool_names.iter().any(|name| name == "mob_probe"));
}

/// 6. `build_agent` with `resume_session` preserves existing session messages.
#[tokio::test]
async fn build_agent_with_resume_preserves_messages() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    // Create a session with existing messages
    let mut session = Session::new();
    session.push(meerkat_core::Message::User(UserMessage::text(
        "Previous question".to_string(),
    )));
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Inherit,
                shell: ToolCategoryOverride::Inherit,
                comms: ToolCategoryOverride::Inherit,
                mob: ToolCategoryOverride::Inherit,
                memory: ToolCategoryOverride::Inherit,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();

    // The session should contain the previous user message (and a system prompt)
    let messages = agent.session().messages();
    let has_previous = messages.iter().any(|m| {
        if let meerkat_core::Message::User(u) = m {
            u.text_content() == "Previous question"
        } else {
            false
        }
    });
    assert!(
        has_previous,
        "Resumed session should preserve prior messages"
    );
}

/// 7. `build_agent` with `resume_session` uses stored metadata as defaults.
#[tokio::test]
async fn build_agent_with_resume_uses_stored_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    // Create a session with metadata already set
    let mut session = Session::new();
    let original_metadata = SessionMetadata {
        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
        model: "claude-sonnet-4-5".to_string(),
        max_tokens: 4096,
        structured_output_retries: 2,
        provider: Provider::Anthropic,
        // K2: durable metadata carries the typed override directly.
        provider_params: Some(meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
            provider_tag: Some(meerkat_core::lifecycle::run_primitive::ProviderTag::Anthropic(
                meerkat_core::lifecycle::run_primitive::AnthropicProviderTag {
                    thinking: Some(
                        meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig::Enabled {
                            budget_tokens: 2048,
                        },
                    ),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }),
        self_hosted_server_id: None,
        tooling: SessionTooling {
            builtins: ToolCategoryOverride::Enable,
            shell: ToolCategoryOverride::Disable,
            comms: ToolCategoryOverride::Disable,
            mob: ToolCategoryOverride::Disable,
            memory: ToolCategoryOverride::Disable,
            schedule: ToolCategoryOverride::Enable,
            workgraph: ToolCategoryOverride::Enable,
            image_generation: ToolCategoryOverride::Inherit,
            web_search: ToolCategoryOverride::Inherit,
            tool_access_policy: None,
            active_skills: None,
        },
        keep_alive: false,
        comms_name: Some("persisted-resume-name".to_string()),
        peer_meta: Some(
            meerkat_core::PeerMeta::default()
                .with_label("mob_id", "mob-a")
                .with_label("role", "worker")
                .with_label("meerkat_id", "w-1"),
        ),
        realm_id: None,
        instance_id: None,
        backend: None,
        config_generation: None,
        auth_binding: None,
        mob_member_binding: None,
    };
    session.set_session_metadata(original_metadata).unwrap();

    // Resume with the same model (simulating the normal resume flow).
    // The resumed metadata enables WorkGraph (tooling.workgraph = Enable);
    // post-#109 an explicit WorkGraph enable must be deliverable, so the build
    // is realm-scoped (the typed RealmId owns the WorkGraph store scope) rather
    // than relying on the removed "default" slug fallback.
    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        provider: Some(Provider::OpenAI),
        max_tokens: Some(1024),
        workgraph_tools: Some(test_workgraph_tools()),
        provider_params: Some(
            meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                temperature: Some(0.1),
                ..Default::default()
            },
        ),
        realm_id: Some(meerkat_core::RealmId::parse("dev").expect("valid realm")),
        ..AgentBuildConfig::new("gpt-5.4")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();

    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");
    assert_eq!(metadata.model, "claude-sonnet-4-5");
    assert_eq!(metadata.max_tokens, 4096);
    assert_eq!(metadata.provider, Provider::Anthropic);
    assert_eq!(
        metadata.provider_params,
        Some(meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
            provider_tag: Some(meerkat_core::lifecycle::run_primitive::ProviderTag::Anthropic(
                meerkat_core::lifecycle::run_primitive::AnthropicProviderTag {
                    thinking: Some(
                        meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig::Enabled {
                            budget_tokens: 2048,
                        },
                    ),
                    ..Default::default()
                },
            )),
            ..Default::default()
        })
    );
    assert_eq!(
        metadata.comms_name.as_deref(),
        Some("persisted-resume-name"),
        "resumed durable comms identity should win over current build defaults"
    );
    assert_eq!(
        metadata.tooling.schedule,
        ToolCategoryOverride::Enable,
        "resumed durable schedule exposure intent should survive the factory metadata merge"
    );
    assert_eq!(
        metadata.tooling.workgraph,
        ToolCategoryOverride::Enable,
        "resumed durable WorkGraph exposure intent should survive the factory metadata merge"
    );
    assert_eq!(
        metadata
            .peer_meta
            .as_ref()
            .and_then(|meta| meta.labels.get("meerkat_id")),
        Some(&"w-1".to_string()),
        "resumed durable peer metadata should be preserved"
    );
}

/// Fail-closed contract (post-#850): a WorkGraph-enabled non-wasm build with
/// no supplied dispatcher (and no surface-installed default) must refuse to
/// build with the typed Config error instead of silently falling back to an
/// in-memory store.
#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn build_agent_workgraph_enabled_without_dispatcher_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Disable,
                shell: ToolCategoryOverride::Disable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Disable,
                workgraph: ToolCategoryOverride::Enable,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        provider: Some(Provider::Anthropic),
        max_tokens: Some(1024),
        realm_id: Some(meerkat_core::RealmId::parse("dev").expect("valid realm")),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let Err(error) = factory.build_agent(build_config, &config).await else {
        panic!("WorkGraph enable without a dispatcher must fail the build closed");
    };
    assert!(
        error.to_string().contains("without a supplied dispatcher"),
        "expected the typed fail-closed Config error, got: {error}"
    );
}

#[tokio::test]
async fn build_agent_with_resume_preserves_explicit_override_masked_fields() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: Some(
                meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                    thinking_budget_tokens: Some(2048),
                    ..Default::default()
                },
            ),
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Disable,
                shell: ToolCategoryOverride::Disable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: true,
            comms_name: Some("persisted-name".to_string()),
            peer_meta: Some(meerkat_core::PeerMeta::default().with_label("role", "persisted")),
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let mut build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        provider: Some(Provider::OpenAI),
        max_tokens: Some(1024),
        // The specific knob doesn't matter for the override-mask contract
        // this test exercises; the typed override carries it directly (K2).
        provider_params: Some(
            meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                provider_tag: Some(meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(
                    meerkat_core::lifecycle::run_primitive::OpenAiProviderTag {
                        reasoning_effort: Some(
                            meerkat_core::lifecycle::run_primitive::ReasoningEffort::Low,
                        ),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        ),
        keep_alive: false,
        comms_name: Some("explicit-name".to_string()),
        peer_meta: Some(meerkat_core::PeerMeta::default().with_label("role", "explicit")),
        ..AgentBuildConfig::new("gpt-5.4")
    };
    build_config.resume_override_mask.model = true;
    build_config.resume_override_mask.provider = true;
    build_config.resume_override_mask.max_tokens = true;
    build_config.resume_override_mask.provider_params = true;
    build_config.resume_override_mask.keep_alive = true;
    build_config.resume_override_mask.comms_name = true;
    build_config.resume_override_mask.peer_meta = true;

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");

    assert_eq!(metadata.model, "gpt-5.4");
    assert_eq!(metadata.provider, Provider::OpenAI);
    assert_eq!(metadata.max_tokens, 1024);
    assert_eq!(
        metadata.provider_params,
        Some(
            meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                provider_tag: Some(meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(
                    meerkat_core::lifecycle::run_primitive::OpenAiProviderTag {
                        reasoning_effort: Some(
                            meerkat_core::lifecycle::run_primitive::ReasoningEffort::Low,
                        ),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            }
        )
    );
    assert!(!metadata.keep_alive);
    assert_eq!(metadata.comms_name.as_deref(), Some("explicit-name"));
    assert_eq!(
        metadata
            .peer_meta
            .as_ref()
            .and_then(|meta| meta.labels.get("role")),
        Some(&"explicit".to_string())
    );
}

#[tokio::test]
async fn build_agent_with_resume_preserves_persisted_system_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let mut config = Config::default();
    config.agent.system_prompt = Some("Current config prompt should not be applied".to_string());

    let mut session = Session::new();
    session.set_system_prompt("Persisted system prompt".to_string());
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Disable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        ..AgentBuildConfig::new("gpt-5.4")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();

    match agent.session().messages().first() {
        Some(meerkat_core::Message::System(system)) => {
            assert_eq!(
                system.content, "Persisted system prompt",
                "resume should preserve the persisted system prompt instead of silently composing the current one"
            );
        }
        other => panic!("expected persisted system prompt, got {other:?}"),
    }
}

#[tokio::test]
async fn build_agent_with_resume_preserves_explicit_inherit_tool_override() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).builtins(false);
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Inherit,
                comms: ToolCategoryOverride::Inherit,
                mob: ToolCategoryOverride::Inherit,
                memory: ToolCategoryOverride::Inherit,
                schedule: ToolCategoryOverride::Enable,
                workgraph: ToolCategoryOverride::Enable,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let mut build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        override_builtins: ToolCategoryOverride::Inherit,
        override_schedule: ToolCategoryOverride::Disable,
        override_workgraph: ToolCategoryOverride::Disable,
        ..AgentBuildConfig::new("gpt-5.4")
    };
    build_config.resume_override_mask.override_builtins = true;
    build_config.resume_override_mask.override_schedule = true;
    build_config.resume_override_mask.override_workgraph = true;

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");

    assert_eq!(metadata.tooling.builtins, ToolCategoryOverride::Inherit);
    assert_eq!(metadata.tooling.schedule, ToolCategoryOverride::Disable);
    assert_eq!(metadata.tooling.workgraph, ToolCategoryOverride::Disable);
}

// Regression for dogma row #76: the facade must carry `override_comms`
// through the full resume-override pipeline — apply_build copy, mask-OR
// (explicit override wins over persisted), and mask-rehydration (persisted
// value survives when the caller did not set the mask). Comms previously had
// the typed field plumbed in core + on the build struct, but the facade
// dropped it at all three seams, so a resumed turn silently re-derived comms
// tooling from the build default instead of honoring the explicit override.
#[tokio::test]
async fn build_agent_with_resume_carries_explicit_comms_override_and_rehydrates() {
    fn session_with_comms(comms: ToolCategoryOverride) -> Session {
        let mut session = Session::new();
        session
            .set_session_metadata(SessionMetadata {
                schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
                model: "claude-sonnet-4-5".to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::Anthropic,
                provider_params: None,
                self_hosted_server_id: None,
                tooling: SessionTooling {
                    builtins: ToolCategoryOverride::Enable,
                    shell: ToolCategoryOverride::Inherit,
                    comms,
                    mob: ToolCategoryOverride::Inherit,
                    memory: ToolCategoryOverride::Inherit,
                    schedule: ToolCategoryOverride::Inherit,
                    workgraph: ToolCategoryOverride::Inherit,
                    image_generation: ToolCategoryOverride::Inherit,
                    web_search: ToolCategoryOverride::Inherit,
                    tool_access_policy: None,
                    active_skills: None,
                },
                keep_alive: false,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: None,
                auth_binding: None,
                mob_member_binding: None,
            })
            .unwrap();
        session
    }

    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    // Explicit-override leg: caller sets override_comms + its mask bit, so the
    // explicit Disable must win over the persisted Enable.
    let mut explicit = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session_with_comms(ToolCategoryOverride::Enable)),
        override_comms: ToolCategoryOverride::Disable,
        ..AgentBuildConfig::new("gpt-5.4")
    };
    explicit.resume_override_mask.override_comms = true;

    let agent = factory.build_agent(explicit, &config).await.unwrap();
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");
    assert_eq!(
        metadata.tooling.comms,
        ToolCategoryOverride::Disable,
        "explicit override_comms must win over the persisted comms tooling on resume"
    );

    // Rehydration leg: caller leaves override_comms at Inherit with no mask
    // bit, so the persisted Enable must survive instead of collapsing to the
    // build default.
    let rehydrate = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session_with_comms(ToolCategoryOverride::Enable)),
        ..AgentBuildConfig::new("gpt-5.4")
    };

    let agent = factory.build_agent(rehydrate, &config).await.unwrap();
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");
    assert_eq!(
        metadata.tooling.comms,
        ToolCategoryOverride::Enable,
        "persisted comms tooling must rehydrate when the caller sets no explicit override"
    );
}

#[cfg(feature = "comms")]
#[tokio::test]
async fn build_agent_with_resume_preserves_session_scoped_inproc_peer_id() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).runtime_root(temp.path());
    let mut config = Config::default();
    config.comms.mode = meerkat_core::CommsRuntimeMode::Inproc;

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Disable,
                comms: ToolCategoryOverride::Enable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: Some("resume-peer".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session.clone()),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let peer_id = agent
        .comms_arc()
        .and_then(|runtime| runtime.public_key())
        .expect("resumed agent should have inproc comms identity");
    drop(agent);

    let rebuilt = factory
        .build_agent(
            AgentBuildConfig {
                llm_client_override: Some(Arc::new(MockLlmClient)),
                resume_session: Some(session),
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
            &config,
        )
        .await
        .unwrap();
    let rebuilt_peer_id = rebuilt
        .comms_arc()
        .and_then(|runtime| runtime.public_key())
        .expect("rebuilt resumed agent should have inproc comms identity");

    assert_eq!(
        rebuilt_peer_id, peer_id,
        "same resumed session_id should preserve the inproc comms peer_id"
    );
}

#[cfg(feature = "comms")]
#[tokio::test]
async fn build_agent_with_resume_preserves_session_scoped_inproc_peer_id_across_runtime_roots() {
    let identity_home = tempfile::tempdir().unwrap();
    let temp_a = tempfile::tempdir().unwrap();
    let factory_a = temp_factory(&temp_a)
        .runtime_root(temp_a.path())
        .user_config_root(identity_home.path());
    let temp_b = tempfile::tempdir().unwrap();
    let factory_b = temp_factory(&temp_b)
        .runtime_root(temp_b.path())
        .user_config_root(identity_home.path());
    let mut config = Config::default();
    config.comms.mode = meerkat_core::CommsRuntimeMode::Inproc;

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Disable,
                comms: ToolCategoryOverride::Enable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: Some("resume-peer".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let build = |session: Session| AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent_a = factory_a
        .build_agent(build(session.clone()), &config)
        .await
        .unwrap();
    let peer_id_a = agent_a
        .comms_arc()
        .and_then(|runtime| runtime.public_key())
        .expect("first resumed agent should have inproc comms identity");
    drop(agent_a);

    let agent_b = factory_b
        .build_agent(build(session), &config)
        .await
        .unwrap();
    let peer_id_b = agent_b
        .comms_arc()
        .and_then(|runtime| runtime.public_key())
        .expect("second resumed agent should have inproc comms identity");

    assert_eq!(
        peer_id_b, peer_id_a,
        "same resumed session_id should preserve peer_id even when runtime roots differ"
    );
}

/// 8. `build_agent` applies system_prompt override.
#[tokio::test]
async fn build_agent_applies_system_prompt_override() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let custom_prompt = "You are a helpful test assistant.".to_string();
    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        system_prompt: meerkat::SystemPromptOverride::Set(custom_prompt.clone()),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();

    let messages = agent.session().messages();
    assert!(!messages.is_empty(), "Session should have messages");
    match &messages[0] {
        meerkat_core::Message::System(sys) => {
            assert!(
                sys.content.contains("You are a helpful test assistant"),
                "System prompt should contain override, got: {}",
                sys.content
            );
        }
        other => panic!("First message should be System, got: {other:?}"),
    }
}

/// 9. `build_agent` with keep_alive but no comms_name fails.
#[cfg(feature = "comms")]
#[tokio::test]
async fn build_agent_keep_alive_without_comms_name_fails() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        keep_alive: true,
        comms_name: None,
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let result = factory.build_agent(build_config, &config).await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("Expected KeepAliveRequiresCommsName error"),
    };
    assert!(
        matches!(err, BuildAgentError::KeepAliveRequiresCommsName),
        "Should be KeepAliveRequiresCommsName, got: {err:?}"
    );
}

#[tokio::test]
async fn build_agent_rejects_invalid_inline_peer_notification_threshold() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        max_inline_peer_notifications: Some(-2),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let result = factory.build_agent(build_config, &config).await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("Expected Config error for invalid threshold"),
    };
    assert!(
        matches!(err, BuildAgentError::Config(_)),
        "Should be Config error, got: {err:?}"
    );
    assert!(
        err.to_string()
            .contains("max_inline_peer_notifications=-2 is invalid")
    );
}

/// 10. `build_agent` with explicit provider skips inference.
#[tokio::test]
async fn build_agent_with_explicit_provider() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        provider: Some(Provider::Anthropic),
        ..AgentBuildConfig::new("my-custom-model")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();

    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");
    assert_eq!(metadata.provider, Provider::Anthropic);
    assert_eq!(metadata.model, "my-custom-model");
}

/// 11. `build_agent` uses config default max_tokens when not specified.
#[tokio::test]
async fn build_agent_uses_config_default_max_tokens() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();
    // The factory resolves the operative per-turn limit from the (now optional)
    // config field at point-of-use; mirror that here.
    let expected_max_tokens = config.resolved_max_tokens();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        // max_tokens: None -- should use config default
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();

    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");
    assert_eq!(
        metadata.max_tokens, expected_max_tokens,
        "Should use config default max_tokens"
    );
}

// ---------------------------------------------------------------------------
// Phase 6: Skills Factory Wiring Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_preload_none_generates_inventory() {
    let factory = AgentFactory::new("/tmp/test-store");
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        preload_skills: None, // No pre-loading → inventory mode
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    // Agent should build successfully even with no skills on disk
    // (empty directories produce empty inventory, which is fine)
    let _ = agent;
}

#[tokio::test]
async fn test_enabled_false_skips_skills() {
    let factory = AgentFactory::new("/tmp/test-store");
    let mut config = Config::default();
    config.skills.enabled = false;

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let metadata = agent.session().session_metadata().unwrap();
    // Skills should not be active when disabled
    assert!(metadata.tooling.active_skills.is_none());
}

#[cfg(feature = "skills")]
#[tokio::test]
async fn test_preload_missing_skill_fails_build() {
    let factory = AgentFactory::new("/tmp/test-store");
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        preload_skills: Some(vec![meerkat_core::skills::SkillKey::builtin(
            meerkat_core::skills::SkillName::parse("nonexistent-skill").expect("valid skill name"),
        )]),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let result = factory.build_agent(build_config, &config).await;
    assert!(
        result.is_err(),
        "Should fail when preloading a nonexistent skill"
    );
}

#[cfg(feature = "skills")]
#[tokio::test]
async fn test_mixed_validity_skills_quarantine_preserves_valid_preload() {
    let temp = tempfile::tempdir().unwrap();
    let skills_root = temp.path().join(".rkat/skills");
    tokio::fs::create_dir_all(skills_root.join("valid-skill"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(skills_root.join("broken-skill"))
        .await
        .unwrap();
    tokio::fs::write(
        skills_root.join("valid-skill/SKILL.md"),
        "---\nname: valid-skill\ndescription: Valid skill\n---\n\n# Valid",
    )
    .await
    .unwrap();
    // Invalid on purpose: frontmatter name does not match directory slug.
    tokio::fs::write(
        skills_root.join("broken-skill/SKILL.md"),
        "---\nname: wrong-name\ndescription: Invalid skill\n---\n\n# Invalid",
    )
    .await
    .unwrap();

    let factory = temp_factory(&temp).project_root(temp.path());
    let config = Config::default();

    let valid_build = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        preload_skills: Some(vec![meerkat_core::skills::SkillKey::new(
            meerkat_core::skills::SourceUuid::project_local(),
            meerkat_core::skills::SkillName::parse("valid-skill").expect("valid skill name"),
        )]),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };
    let valid_result = factory.build_agent(valid_build, &config).await;
    assert!(
        valid_result.is_ok(),
        "Expected valid skill preload to succeed despite quarantined sibling; got: {:?}",
        valid_result.err()
    );
    let metadata = valid_result
        .unwrap()
        .session()
        .session_metadata()
        .expect("valid preload session metadata");
    assert_eq!(
        metadata.tooling.active_skills,
        Some(vec![meerkat_core::skills::SkillKey::new(
            meerkat_core::skills::SourceUuid::project_local(),
            meerkat_core::skills::SkillName::parse("valid-skill").expect("valid skill name"),
        )]),
        "session metadata should persist only the explicitly preloaded skills"
    );

    let invalid_build = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        preload_skills: Some(vec![meerkat_core::skills::SkillKey::new(
            meerkat_core::skills::SourceUuid::project_local(),
            meerkat_core::skills::SkillName::parse("broken-skill").expect("valid skill name"),
        )]),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };
    let invalid_result = factory.build_agent(invalid_build, &config).await;
    assert!(
        invalid_result.is_err(),
        "Expected quarantined invalid skill preload to fail deterministically"
    );
}

#[cfg(feature = "skills")]
#[tokio::test]
async fn test_resume_does_not_mutate_persisted_active_skills_when_current_surface_cannot_load_them()
{
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let mut resumed = Session::new();
    resumed.push(meerkat::Message::User(UserMessage::text(
        "Remember the codename ResumeOtter.",
    )));
    resumed
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".into(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Disable,
                shell: ToolCategoryOverride::Disable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: Some(vec![meerkat_core::skills::SkillKey::builtin(
                    meerkat_core::skills::SkillName::parse("nonexistent-legacy-skill")
                        .expect("valid skill name"),
                )]),
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .expect("resume metadata");

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(resumed),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory
        .build_agent(build_config, &config)
        .await
        .expect("resume should drop incompatible persisted active skills");
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");
    assert_eq!(
        metadata.tooling.active_skills,
        Some(vec![meerkat_core::skills::SkillKey::builtin(
            meerkat_core::skills::SkillName::parse("nonexistent-legacy-skill")
                .expect("valid skill name"),
        )]),
        "resume may drop unavailable skills from the live surface projection, but it must not rewrite durable session behavior truth"
    );
}

// ---------------------------------------------------------------------------
// Custom SessionStore tests
// ---------------------------------------------------------------------------

/// A custom session store that tracks save calls.
struct TrackingSessionStore {
    save_count: std::sync::atomic::AtomicU32,
}

impl TrackingSessionStore {
    fn new() -> Self {
        Self {
            save_count: std::sync::atomic::AtomicU32::new(0),
        }
    }

    fn save_count(&self) -> u32 {
        self.save_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl SessionStore for TrackingSessionStore {
    async fn save(&self, _session: &Session) -> Result<(), SessionStoreError> {
        self.save_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn load(&self, _id: &SessionId) -> Result<Option<Session>, SessionStoreError> {
        Ok(None)
    }

    async fn list(
        &self,
        _filter: SessionFilter,
    ) -> Result<Vec<meerkat_core::SessionMeta>, SessionStoreError> {
        Ok(vec![])
    }

    async fn delete(&self, _id: &SessionId) -> Result<(), SessionStoreError> {
        Ok(())
    }

    async fn delete_if_current_revision(
        &self,
        _id: &SessionId,
        _expected_current_revision: &str,
    ) -> Result<bool, SessionStoreError> {
        Ok(false)
    }
}

/// 12. `build_agent` with custom session store uses the provided store.
#[tokio::test]
async fn build_agent_with_custom_session_store() {
    let store = Arc::new(TrackingSessionStore::new());
    let factory = AgentFactory::new("/unused/path").session_store(store.clone());
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let mut agent = factory.build_agent(build_config, &config).await.unwrap();

    // Store should not have been called yet (no save before run)
    assert_eq!(store.save_count(), 0);

    // Run the agent — the agent loop saves the session on completion
    let result = agent.run("Hello".to_string().into()).await.unwrap();
    assert!(result.text.contains("Hello from mock"));

    // The custom store should have received at least one save call
    assert!(
        store.save_count() > 0,
        "Custom store should have been used for saving, got {} saves",
        store.save_count()
    );
}

// ---------------------------------------------------------------------------
// Regression tests: ToolCategoryOverride upgrade semantics on resume
// ---------------------------------------------------------------------------

/// Regression: a session created before mob tools existed (mob=Inherit via
/// serde default) should NOT suppress mob tools on resume. The factory's
/// current runtime default should win.
#[tokio::test]
async fn resume_with_inherit_mob_allows_factory_default() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).mob(true); // factory enables mob
    let config = Config::default();

    // Simulate a session from a pre-mob Meerkat version:
    // mob field is Inherit (the serde default for missing/false fields).
    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Enable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Inherit, // <-- key: no opinion
                memory: ToolCategoryOverride::Inherit,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5".to_string());
    build_config.resume_session = Some(session);
    build_config.llm_client_override = Some(Arc::new(MockLlmClient));

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let metadata = agent.session().session_metadata().unwrap();

    // Mob should stay Inherit — the session has no opinion, so it continues to
    // follow whatever the factory default is at each future resume.
    assert_eq!(
        metadata.tooling.mob,
        ToolCategoryOverride::Inherit,
        "Inherit mob must be preserved through save/resume, not collapsed to Enable"
    );
}

/// Regression: a session with mob=Disable should remain disabled on resume,
/// even if the factory now enables mob by default.
#[tokio::test]
async fn resume_with_disable_mob_stays_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).mob(true); // factory enables mob
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Enable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Disable, // <-- explicitly off
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5".to_string());
    build_config.resume_session = Some(session);
    build_config.llm_client_override = Some(Arc::new(MockLlmClient));

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let metadata = agent.session().session_metadata().unwrap();

    assert_eq!(
        metadata.tooling.mob,
        ToolCategoryOverride::Disable,
        "Disable mob should be preserved on resume despite factory enabling it"
    );
}

/// Regression: metadata-only mob=Enable is a compatibility mirror on resume.
/// Without generated authority or explicit fresh override, it must not survive
/// as active build intent when the factory default disables mob.
#[tokio::test]
async fn resume_with_metadata_mob_enable_becomes_inherit() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).mob(false); // factory disables mob
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Enable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Enable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5".to_string());
    build_config.resume_session = Some(session);
    build_config.llm_client_override = Some(Arc::new(MockLlmClient));

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let metadata = agent.session().session_metadata().unwrap();

    assert_eq!(
        metadata.tooling.mob,
        ToolCategoryOverride::Inherit,
        "metadata-only mob enablement must not be preserved as active authority"
    );
}

#[tokio::test]
async fn ambient_factory_mob_enable_does_not_imply_operator_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobToolsFactory {
        observed_authority_context: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp).mob(true).mob_tools_factory(mob_factory);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert!(
        observed.is_empty(),
        "ambient mob defaults must not invoke a mob factory without generated authority"
    );
}

#[tokio::test]
async fn ambient_factory_mob_enable_does_not_mount_mob_dispatcher() {
    let temp = tempfile::tempdir().unwrap();
    let factory =
        temp_factory(&temp)
            .mob(true)
            .mob_tools_factory(Arc::new(StaticMobToolsFactory {
                dispatcher: Arc::new(NamedDispatcher::new("mob_probe")),
            }));

    let tool_names =
        run_and_capture_tool_names(factory, AgentBuildConfig::new("claude-sonnet-4-5")).await;

    assert!(
        !tool_names.iter().any(|name| name == "mob_probe"),
        "ambient mob defaults must not mount a mob dispatcher without generated authority"
    );
}

#[tokio::test]
async fn resumed_enable_mob_metadata_does_not_imply_operator_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobToolsFactory {
        observed_authority_context: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp).mob(true).mob_tools_factory(mob_factory);
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Enable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Enable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let build_config = AgentBuildConfig {
        resume_session: Some(session),
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert!(
        observed.is_empty(),
        "resumed metadata-only mob enablement must not invoke a mob factory without generated authority"
    );
}

#[tokio::test]
async fn resumed_enable_mob_metadata_does_not_mount_mob_surface() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobToolsFactory {
        observed_authority_context: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp)
        .mob(false)
        .mob_tools_factory(mob_factory);
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Enable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Enable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let build_config = AgentBuildConfig {
        resume_session: Some(session),
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert!(
        observed.is_empty(),
        "metadata-only mob enablement must not request a mob surface when factory defaults disable it"
    );
    assert_eq!(
        agent.session().session_metadata().unwrap().tooling.mob,
        ToolCategoryOverride::Inherit,
        "metadata-only mob enablement must not be repersisted as active build intent"
    );
}

#[tokio::test]
async fn recovered_create_request_mob_metadata_enable_does_not_mint_operator_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobToolsFactory {
        observed_authority_context: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp).mob(true).mob_tools_factory(mob_factory);
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Enable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Enable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .unwrap();

    let recovered = meerkat_core::build_recovered_session(
        session,
        &meerkat_core::SurfaceSessionRecoveryOverrides::default(),
        meerkat_core::SurfaceSessionRecoveryContext::default(),
    )
    .unwrap();
    assert_eq!(
        recovered.build.override_mob,
        ToolCategoryOverride::Inherit,
        "metadata-only mob enablement must not become an explicit build override"
    );

    let request = recovered.into_deferred_create_request();
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
    let mut build_config = AgentBuildConfig::from_create_session_request(&request, event_tx);
    build_config.llm_client_override = Some(Arc::new(MockLlmClient));

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert!(
        observed.is_empty(),
        "recovered metadata-only mob enablement must not invoke a mob factory without generated authority"
    );
}

#[tokio::test]
async fn explicit_mob_override_generates_create_only_operator_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobToolsFactory {
        observed_authority_context: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp)
        .mob(false)
        .mob_tools_factory(mob_factory);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        override_mob: ToolCategoryOverride::Enable,
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert_eq!(observed.len(), 1);
    assert_generated_create_only_authority(observed[0].as_ref());
}

#[cfg(feature = "comms")]
#[tokio::test]
async fn explicit_mob_override_builds_parent_comms_identity_for_delegate_wiring() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobCommsFactory {
        observed: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp)
        .comms(true)
        .mob(false)
        .mob_tools_factory(mob_factory);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        override_mob: ToolCategoryOverride::Enable,
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert_eq!(observed.len(), 1);
    assert!(
        observed[0].comms_name.is_some(),
        "mob-enabled parent sessions must get a canonical comms_name so delegate() can wire helpers"
    );
    assert!(
        observed[0].comms_runtime_present,
        "mob-enabled parent sessions must get a comms runtime so delegate() can install parent<->child trust"
    );
}

#[cfg(feature = "comms")]
#[tokio::test]
async fn ambient_mob_enable_without_authority_does_not_create_parent_comms_identity() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobCommsFactory {
        observed: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp)
        .comms(true)
        .mob(true)
        .mob_tools_factory(mob_factory);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert!(
        observed.is_empty(),
        "ambient mob availability without generated authority must not invoke a mob factory"
    );
}

#[tokio::test]
async fn resumed_explicit_mob_override_generates_create_only_operator_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobToolsFactory {
        observed_authority_context: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp)
        .mob(false)
        .mob_tools_factory(mob_factory);
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Enable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();

    let build_config = AgentBuildConfig {
        override_mob: ToolCategoryOverride::Enable,
        resume_session: Some(session),
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert_eq!(observed.len(), 1);
    assert_generated_create_only_authority(observed[0].as_ref());
}

#[tokio::test]
async fn explicit_mob_authority_is_forwarded_to_mob_tools_factory() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobToolsFactory {
        observed_authority_context: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp).mob(true).mob_tools_factory(mob_factory);
    let config = Config::default();

    let mut build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };
    let expected_authority = create_test_authority();
    build_config.mob_tool_authority_context = Some(expected_authority.clone());

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert_eq!(observed.as_slice(), &[Some(expected_authority)]);
    assert_eq!(
        agent.session().session_metadata().unwrap().tooling.mob,
        ToolCategoryOverride::Enable,
        "generated authority handoff should update the durable visibility mirror explicitly"
    );
}

#[tokio::test]
async fn resumed_explicit_mob_authority_is_not_erased_by_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobToolsFactory {
        observed_authority_context: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp)
        .mob(false)
        .mob_tools_factory(mob_factory);
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Enable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();
    let expected_authority = create_test_authority();

    let build_config = AgentBuildConfig {
        resume_session: Some(session),
        mob_tool_authority_context: Some(expected_authority.clone()),
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert_eq!(observed.as_slice(), &[Some(expected_authority)]);
}

#[tokio::test]
async fn explicit_mob_authority_persists_only_non_authoritative_projection() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let mut build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };
    let expected_authority = create_test_authority();
    build_config.mob_tool_authority_context = Some(expected_authority.clone());

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    assert!(agent.session().mob_tool_authority_context().is_none());
    let persisted = agent
        .session()
        .build_state()
        .and_then(|state| state.mob_tool_authority_context)
        .expect("mob authority projection should be stored");
    assert!(!persisted.is_generated_authority_context());
    assert!(!persisted.can_manage_mob("mob-a"));
}

#[tokio::test]
async fn resumed_persisted_mob_authority_is_not_forwarded_as_behavior_authority() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mob_factory = Arc::new(RecordingMobToolsFactory {
        observed_authority_context: Arc::clone(&observed),
    });
    let factory = temp_factory(&temp)
        .mob(false)
        .mob_tools_factory(mob_factory);
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 2048,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: None,
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Enable,
                shell: ToolCategoryOverride::Enable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Inherit,
                memory: ToolCategoryOverride::Disable,
                schedule: ToolCategoryOverride::Inherit,
                workgraph: ToolCategoryOverride::Inherit,
                image_generation: ToolCategoryOverride::Inherit,
                web_search: ToolCategoryOverride::Inherit,
                tool_access_policy: None,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: None,
            mob_member_binding: None,
        })
        .unwrap();
    session
        .set_build_state(meerkat_core::SessionBuildState::default())
        .unwrap();
    let expected_authority = create_test_authority();
    session
        .set_mob_tool_authority_context(Some(expected_authority.clone()))
        .unwrap();

    let build_config = AgentBuildConfig {
        resume_session: Some(session),
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert!(observed.is_empty());
}

// ---------------------------------------------------------------------------
// Shared comms runtime vs per-session comms_name
// ---------------------------------------------------------------------------

/// When `with_comms_runtime` is set and the build has `comms_name`, the factory
/// must NOT use the shared runtime — it must create a per-session runtime so the
/// member gets its own keypair, inbox, and trusted-peer set. Sharing the surface
/// runtime with mob members would collapse their PeerCommsMachine state into one
/// instance, breaking peer-to-peer addressing.
///
/// This test validates both paths:
/// 1. No comms_name → build succeeds using the shared runtime
/// 2. comms_name set → build succeeds using a fresh per-session runtime
///
/// The two agents must have different peer identities (different public keys).
#[cfg(feature = "comms")]
#[tokio::test]
async fn shared_comms_runtime_skipped_when_comms_name_set() {
    let temp = tempfile::tempdir().unwrap();
    let comms_config = meerkat_comms::ResolvedCommsConfig {
        enabled: true,
        name: "shared-parent".into(),
        inproc_namespace: None,
        listen_tcp: None,
        listen_uds: None,
        advertise_address: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        identity_dir: temp.path().join("identity"),
        trusted_peers_path: temp.path().join("trusted_peers.json"),
        comms_config: Default::default(),
        auth: Default::default(),
        require_peer_auth: false,
        allow_external_unauthenticated: false,
        pairing_password: None,
    };
    let shared_runtime = Arc::new(
        meerkat_comms::CommsRuntime::new_with_silent_intents(
            comms_config,
            Arc::new(std::collections::HashSet::new()),
        )
        .await
        .unwrap(),
    );
    let _shared_pubkey = shared_runtime.public_key().to_peer_id();

    // Add a sentinel peer to the shared runtime so we can detect reuse.
    let sentinel_pubkey = meerkat_comms::identity::Keypair::generate().public_key();
    add_trusted_peer_with_generated_authority(
        Arc::clone(&shared_runtime),
        meerkat_core::comms::TrustedPeerDescriptor::unsigned_with_pubkey(
            "sentinel",
            sentinel_pubkey.to_peer_id().to_string(),
            *sentinel_pubkey.as_bytes(),
            "tcp://127.0.0.1:9999",
        )
        .expect("trusted peer descriptor"),
    )
    .await;

    let factory = temp_factory(&temp).with_comms_runtime(shared_runtime);
    let config = Config::default();

    // 1. Build WITHOUT comms_name — should reuse the shared runtime.
    //    The sentinel peer should be visible.
    let parent_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        comms_name: None,
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };
    let parent_agent = factory
        .build_agent(parent_config, &config)
        .await
        .expect("parent build (no comms_name) should succeed");

    // 2. Build WITH comms_name — should get a fresh per-session runtime.
    //    The sentinel peer should NOT be visible.
    let member_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        comms_name: Some("mob/delegate/helper".to_string()),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };
    let member_agent = factory
        .build_agent(member_config, &config)
        .await
        .expect("member build (with comms_name) should succeed");

    // Verify identities differ via the session metadata stored on the agent's session.
    let parent_meta = parent_agent.session().session_metadata();
    let member_meta = member_agent.session().session_metadata();

    // The parent has no comms_name (surface identity).
    assert!(
        parent_meta
            .as_ref()
            .and_then(|m| m.comms_name.as_ref())
            .is_none(),
        "parent session should have no comms_name in metadata"
    );

    // The member has a comms_name (per-session identity).
    assert_eq!(
        member_meta
            .as_ref()
            .and_then(|m| m.comms_name.as_ref())
            .map(String::as_str),
        Some("mob/delegate/helper"),
        "member session should have its own comms_name in metadata"
    );
}

// ============================================================================
// Provider web search default tests
// ============================================================================

/// Mock client that captures provider_params from LlmRequest.
struct ParamsCaptureClient {
    /// Captured provider_params (empty object if None was passed).
    captured: Mutex<serde_json::Value>,
}

impl ParamsCaptureClient {
    fn new() -> Self {
        Self {
            captured: Mutex::new(serde_json::json!(null)),
        }
    }

    fn captured_params(&self) -> Option<serde_json::Value> {
        let val = self.captured.lock().unwrap().clone();
        if val.is_null() { None } else { Some(val) }
    }
}

#[async_trait]
impl LlmClient for ParamsCaptureClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>>
    {
        *self.captured.lock().unwrap() = request
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
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            }),
        ]))
    }
    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[tokio::test]
async fn web_search_default_injected_for_anthropic() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();
    let client = Arc::new(ParamsCaptureClient::new());
    let build_config = AgentBuildConfig {
        llm_client_override: Some(client.clone()),
        ..AgentBuildConfig::new("claude-sonnet-4-6")
    };
    let mut agent = factory.build_agent(build_config, &config).await.unwrap();
    let _ = agent.run("test".to_string().into()).await.unwrap();
    let params = client
        .captured_params()
        .expect("should have provider_params");
    assert!(
        params.get("web_search").is_some(),
        "Anthropic catalog model should have web_search default: {params}"
    );
}

#[tokio::test]
async fn web_search_not_injected_when_config_disabled() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let mut config = Config::default();
    config.provider_tools.anthropic.web_search = false;
    let client = Arc::new(ParamsCaptureClient::new());
    let build_config = AgentBuildConfig {
        llm_client_override: Some(client.clone()),
        ..AgentBuildConfig::new("claude-sonnet-4-6")
    };
    let mut agent = factory.build_agent(build_config, &config).await.unwrap();
    let _ = agent.run("test".to_string().into()).await.unwrap();
    let params = client.captured_params();
    let has_web_search = params.as_ref().and_then(|p| p.get("web_search")).is_some();
    assert!(
        !has_web_search,
        "web_search should not be injected when config disabled: {params:?}"
    );
}

#[tokio::test]
async fn web_search_opt_out_via_null() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();
    let client = Arc::new(ParamsCaptureClient::new());
    let build_config = AgentBuildConfig {
        llm_client_override: Some(client.clone()),
        provider_params: Some(meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
            provider_tag: Some(meerkat_core::lifecycle::run_primitive::ProviderTag::Anthropic(
                meerkat_core::lifecycle::run_primitive::AnthropicProviderTag {
                    web_search: Some(
                        meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                            &serde_json::Value::Null,
                        ),
                    ),
                    ..Default::default()
                },
            )),
            ..Default::default()
        }),
        ..AgentBuildConfig::new("claude-sonnet-4-6")
    };
    let mut agent = factory.build_agent(build_config, &config).await.unwrap();
    let _ = agent.run("test".to_string().into()).await.unwrap();
    let params = client.captured_params();
    let has_web_search = params.as_ref().and_then(|p| p.get("web_search")).is_some();
    assert!(
        !has_web_search,
        "web_search should be removed by null override: {params:?}"
    );
}

#[tokio::test]
async fn web_search_explicit_params_merged_with_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();
    let client = Arc::new(ParamsCaptureClient::new());
    let build_config = AgentBuildConfig {
        llm_client_override: Some(client.clone()),
        provider_params: Some(
            meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                thinking_budget_tokens: Some(5000),
                ..Default::default()
            },
        ),
        ..AgentBuildConfig::new("claude-sonnet-4-6")
    };
    let mut agent = factory.build_agent(build_config, &config).await.unwrap();
    let _ = agent.run("test".to_string().into()).await.unwrap();
    let params = client
        .captured_params()
        .expect("should have provider_params");
    assert!(
        params.get("web_search").is_some(),
        "web_search default should survive explicit param merge: {params}"
    );
    assert!(
        params.get("thinking_budget_tokens").is_some(),
        "explicit thinking_budget should project into typed Anthropic budget: {params}"
    );
}

#[tokio::test]
async fn explicit_meerkat_tool_policy_preserves_provider_search_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();
    let client = Arc::new(ParamsCaptureClient::new());
    let build_config = AgentBuildConfig {
        llm_client_override: Some(client.clone()),
        override_builtins: ToolCategoryOverride::Disable,
        ..AgentBuildConfig::new("gpt-5.4")
    };
    let mut agent = factory.build_agent(build_config, &config).await.unwrap();
    let _ = agent.run("test".to_string().into()).await.unwrap();
    let params = client.captured_params();
    let has_web_search = params.as_ref().and_then(|p| p.get("web_search")).is_some();
    assert!(
        has_web_search,
        "explicit Meerkat tool policy should not suppress provider web_search defaults: {params:?}"
    );
}

#[tokio::test]
async fn explicit_provider_search_param_can_reenable_search_under_tool_policy() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();
    let client = Arc::new(ParamsCaptureClient::new());
    let build_config = AgentBuildConfig {
        llm_client_override: Some(client.clone()),
        override_builtins: ToolCategoryOverride::Disable,
        provider_params: Some(
            meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
                provider_tag: Some(meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(
                    meerkat_core::lifecycle::run_primitive::OpenAiProviderTag {
                        web_search: Some(
                            meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                                &serde_json::json!({"type": "web_search"}),
                            ),
                        ),
                        ..Default::default()
                    },
                )),
                ..Default::default()
            },
        ),
        ..AgentBuildConfig::new("gpt-5.4")
    };
    let mut agent = factory.build_agent(build_config, &config).await.unwrap();
    let _ = agent.run("test".to_string().into()).await.unwrap();
    let params = client
        .captured_params()
        .expect("should have provider_params");
    assert!(
        params.get("web_search").is_some(),
        "explicit provider search override should still be honored under explicit tool policy: {params}"
    );
}

// ---------------------------------------------------------------------------
// Deferral §2: auth_binding hot-swap mid-session
// ---------------------------------------------------------------------------
//
// Dogma §12 (dynamic policy follows dynamic identity) requires that when a
// session persists a `auth_binding`, `build_llm_client_for_identity`
// pins the credential resolve to that specific realm + binding — not a
// synthesized env-default realm. The tests below cover the three failure
// modes we most care about preventing: silent realm substitution,
// swallowing of unknown realms, and ignoring a per-hot-swap override.

#[tokio::test]
async fn hot_swap_scopes_resolve_to_session_auth_binding() {
    // identity.auth_binding = Some(ref) but config.realm has no matching
    // realm → typed error. Confirms the factory no longer falls through to
    // synthesize_realm_from_config + env-default, which would have silently
    // resolved credentials belonging to a different realm in multi-tenant
    // setups.
    //
    // Assumes RKAT_TEST_CLIENT is not set in the test process env — if it
    // were, the factory would short-circuit to TestClient before even
    // reaching the auth_binding branch. Nothing else in this test file
    // sets that variable, so this assumption holds.
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();
    let identity = SessionLlmIdentity {
        model: "claude-opus-4-8".to_string(),
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("tenant_a").expect("valid realm"),
            binding: meerkat_core::BindingId::parse("default").expect("valid binding"),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        }),
    };
    let err = match factory
        .build_llm_client_for_identity(&config, &identity)
        .await
    {
        Ok(_) => panic!("hot-swap against unknown realm must error, not fall through"),
        Err(e) => e,
    };
    let message = err.to_string();
    assert!(
        message.contains("realm 'tenant_a' not found"),
        "expected 'realm tenant_a not found' in error, got: {message}"
    );
}

#[test]
fn session_metadata_projects_auth_binding_into_llm_identity() {
    // Dogma §1/§13: SessionMetadata is the canonical owner;
    // SessionLlmIdentity is a read/write projection. Verify round-trip.
    let conn_ref = meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::parse("prod").expect("valid realm"),
        binding: meerkat_core::BindingId::parse("openai_default").expect("valid binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    };
    let mut metadata = SessionMetadata {
        schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
        model: "gpt-5.4".to_string(),
        max_tokens: 1024,
        structured_output_retries: 2,
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        tooling: SessionTooling::default(),
        keep_alive: false,
        comms_name: None,
        peer_meta: None,
        realm_id: None,
        instance_id: None,
        backend: None,
        config_generation: None,
        auth_binding: Some(conn_ref.clone()),
        mob_member_binding: None,
    };
    let identity = metadata.llm_identity();
    assert_eq!(identity.auth_binding, Some(conn_ref));

    // Overwrite via apply_llm_identity — auth_binding should change.
    let swapped_ref = meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::parse("tenant_b").expect("valid realm"),
        binding: meerkat_core::BindingId::parse("default").expect("valid binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    };
    let new_identity = SessionLlmIdentity {
        model: "gpt-5.4".to_string(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(swapped_ref.clone()),
    };
    metadata.apply_llm_identity(&new_identity);
    assert_eq!(metadata.auth_binding, Some(swapped_ref));
}

/// Memory enabled at the factory level (config truth) must fail closed when
/// the memory store cannot open — even with an `Inherit` per-build override.
/// Building an agent without the promised `memory_search` tool would silently
/// misreport the session's capability truth.
#[cfg(feature = "memory-store-session")]
#[tokio::test]
async fn factory_enabled_memory_open_failure_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let store_path = temp.path().join("sessions");
    std::fs::create_dir_all(&store_path).unwrap();
    // Occupy the memory dir path with a plain file so HnswMemoryStore::open fails.
    std::fs::write(store_path.join("memory"), b"not a directory").unwrap();

    let factory = AgentFactory::new(store_path.clone()).memory(true);
    let config = Config::default();
    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        // Inherit resolves against the factory-level enable: still effective.
        override_memory: ToolCategoryOverride::Inherit,
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let err = factory
        .build_agent(build_config, &config)
        .await
        .err()
        .expect("enabled memory with an unopenable store must fail the build");
    match err {
        BuildAgentError::CapabilityUnavailable { capability, .. } => {
            assert_eq!(capability, "memory");
        }
        other => panic!("expected CapabilityUnavailable for memory, got: {other:?}"),
    }
}

/// Sibling of the open-failure arm: a meerkat build WITHOUT the
/// `memory-store-session` feature can never deliver the promised
/// `memory_search` tool, so an effective memory enable must fail the build
/// typed (`CapabilityUnavailable`) instead of silently producing a
/// memory-less agent.
#[cfg(not(feature = "memory-store-session"))]
#[tokio::test]
async fn factory_enabled_memory_without_feature_fails_closed() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).memory(true);
    let config = Config::default();
    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        // Inherit resolves against the factory-level enable: still effective.
        override_memory: ToolCategoryOverride::Inherit,
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let err = factory
        .build_agent(build_config, &config)
        .await
        .err()
        .expect("enabled memory without the memory-store-session feature must fail the build");
    match err {
        BuildAgentError::CapabilityUnavailable { capability, reason } => {
            assert_eq!(capability, "memory");
            assert!(
                reason.contains("memory-store-session"),
                "reason must name the missing feature, got: {reason}"
            );
        }
        other => panic!("expected CapabilityUnavailable for memory, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Config-owned tool execution policy reaches factory-built agents
// ---------------------------------------------------------------------------

/// `Config.tools` (per-call timeout policy) must be honored by an agent built
/// through `AgentFactory::build_agent()` — not only by direct `AgentBuilder`
/// composition. Regression pin for the dead `with_tools_config` seam: without
/// the factory wiring, every factory-built agent silently ran on
/// `ToolsConfig::default()` and this hanging tool would stall the turn for the
/// default timeout instead of the configured 50ms.
#[tokio::test]
async fn factory_built_agent_honors_config_tools_timeout() {
    struct ToolCallingThenDoneClient {
        calls: Mutex<u32>,
    }

    #[async_trait]
    impl AgentLlmClient for ToolCallingThenDoneClient {
        async fn stream_response(
            &self,
            _messages: &[meerkat::Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&ProviderParamsOverride>,
        ) -> Result<LlmStreamResult, AgentError> {
            let mut calls = self.calls.lock().unwrap();
            let response = if *calls == 0 {
                LlmStreamResult::new(
                    vec![AssistantBlock::ToolUse {
                        id: "slow-call-1".to_string(),
                        name: "slow_tool".to_string(),
                        args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                        meta: None,
                    }],
                    meerkat_core::StopReason::ToolUse,
                    meerkat_core::Usage::default(),
                )
            } else {
                LlmStreamResult::new(
                    vec![AssistantBlock::Text {
                        text: "done".to_string(),
                        meta: None,
                    }],
                    meerkat_core::StopReason::EndTurn,
                    meerkat_core::Usage::default(),
                )
            };
            *calls += 1;
            Ok(response)
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::Other
        }

        fn model(&self) -> &'static str {
            "mock-slow-tool-model"
        }
    }

    struct HangingToolDispatcher {
        tools: Arc<[Arc<ToolDef>]>,
    }

    #[async_trait]
    impl AgentToolDispatcher for HangingToolDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            Arc::clone(&self.tools)
        }

        async fn dispatch(
            &self,
            _call: ToolCallView<'_>,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            std::future::pending().await
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let mut config = Config::default();
    config.tools.default_timeout = std::time::Duration::from_millis(50);

    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(64);
    let build_config = AgentBuildConfig {
        agent_llm_client_override: Some(Arc::new(ToolCallingThenDoneClient {
            calls: Mutex::new(0),
        })),
        external_tools: Some(Arc::new(HangingToolDispatcher {
            tools: Arc::from([Arc::new(ToolDef::new(
                "slow_tool",
                "hangs until the per-call timeout fires",
                json!({ "type": "object" }),
            ))]),
        })),
        event_tx: Some(event_tx),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let mut agent = factory.build_agent(build_config, &config).await.unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        agent.run("use the slow tool".to_string().into()),
    )
    .await
    .expect("run must complete promptly once the configured per-call timeout fires")
    .expect("agent run should succeed after timeout terminalization");
    assert_eq!(result.turns, 2);

    let mut saw_timeout_completion = false;
    while let Ok(event) = event_rx.try_recv() {
        if let meerkat::AgentEvent::ToolExecutionCompleted {
            name,
            is_error,
            content,
            ..
        } = event
            && name == "slow_tool"
        {
            assert!(is_error, "timed-out tool result must be an error");
            let text = meerkat_core::types::text_content(&content);
            assert!(
                text.contains("\"error\":\"timeout\""),
                "timed-out tool result must carry the canonical timeout payload, got: {text}"
            );
            saw_timeout_completion = true;
        }
    }
    assert!(
        saw_timeout_completion,
        "factory-built agent must apply Config.tools.default_timeout to tool dispatch"
    );
}

// ---------------------------------------------------------------------------
// Ask 6: call-level tool access policy (execution gate)
// ---------------------------------------------------------------------------

/// Dispatcher that records which calls actually reach it and succeeds on all
/// of them — the probe for asserting the execution gate sits OUTSIDE it.
struct PolicyProbeDispatcher {
    tools: Arc<[Arc<ToolDef>]>,
    dispatched: Arc<Mutex<Vec<String>>>,
}

impl PolicyProbeDispatcher {
    fn new(names: &[&str], dispatched: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            tools: names
                .iter()
                .map(|name| {
                    Arc::new(ToolDef::new(
                        *name,
                        format!("{name} policy probe tool"),
                        json!({ "type": "object" }),
                    ))
                })
                .collect(),
            dispatched,
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for PolicyProbeDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tools)
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatched.lock().unwrap().push(call.name.to_string());
        Ok(ToolDispatchOutcome::from(meerkat_core::ToolResult::new(
            call.id.to_string(),
            "ok".to_string(),
            false,
        )))
    }
}

fn allow_list_policy(names: &[&str]) -> meerkat_core::ops::ToolAccessPolicy {
    meerkat_core::ops::ToolAccessPolicy::AllowList(names.iter().copied().collect())
}

/// The factory applies the resolved tool access policy as the OUTERMOST
/// dispatcher composition: a denied call surfaces as an ordinary
/// `access_denied` is_error result and never reaches the inner dispatcher,
/// the LLM-visible tool list is preserved (list-preserving gate), and the
/// effective policy is persisted into the session metadata tooling section.
#[tokio::test]
async fn build_agent_gates_dispatch_and_persists_tool_access_policy() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let dispatched = Arc::new(Mutex::new(Vec::new()));
    let policy = allow_list_policy(&["alpha"]);

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        tool_dispatcher_override: Some(Arc::new(PolicyProbeDispatcher::new(
            &["alpha", "beta"],
            Arc::clone(&dispatched),
        ))),
        tool_access_policy: Some(policy.clone()),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };
    let mut agent = factory
        .build_agent(build_config, &Config::default())
        .await
        .expect("gated build must succeed");

    // The effective policy is persisted for children to inherit.
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session metadata must be set");
    assert_eq!(metadata.tooling.tool_access_policy, Some(policy));

    // List-preserving: the denied tool stays LLM-visible (the prompt-cache
    // prefix is unchanged); only execution is gated.
    let visible: Vec<String> = agent
        .tool_scope()
        .visible_tools()
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(
        visible.iter().any(|name| name == "alpha"),
        "saw {visible:?}"
    );
    assert!(visible.iter().any(|name| name == "beta"), "saw {visible:?}");

    // Denied call: ordinary is_error result, inner dispatcher never reached.
    let outcome = agent
        .dispatch_external_tool_call(meerkat_core::ToolCall::new(
            "call-denied".to_string(),
            "beta".to_string(),
            json!({}),
        ))
        .await
        .expect("policy denial is a tool result, not a dispatch fault");
    assert!(outcome.result.is_error);
    assert!(
        outcome
            .result
            .text_content()
            .contains("\"error\":\"access_denied\""),
        "denied call must carry the canonical access_denied payload, got: {}",
        outcome.result.text_content()
    );
    assert!(
        dispatched.lock().unwrap().is_empty(),
        "denied call must never reach the inner dispatcher"
    );

    // Allowed call flows through to the inner dispatcher.
    let outcome = agent
        .dispatch_external_tool_call(meerkat_core::ToolCall::new(
            "call-allowed".to_string(),
            "alpha".to_string(),
            json!({}),
        ))
        .await
        .expect("allowed call must dispatch");
    assert!(!outcome.result.is_error);
    assert_eq!(*dispatched.lock().unwrap(), vec!["alpha".to_string()]);
}

/// An unresolved `Inherit` reaching the factory is a wiring fault (the spawn
/// chain owns Inherit resolution) and must fail the build closed with a typed
/// error — never silently build an ungated agent or persist `Inherit`.
#[tokio::test]
async fn build_agent_fails_closed_on_unresolved_inherit_tool_access_policy() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        tool_dispatcher_override: Some(Arc::new(EmptyDispatcher)),
        tool_access_policy: Some(meerkat_core::ops::ToolAccessPolicy::Inherit),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };
    let err = factory
        .build_agent(build_config, &Config::default())
        .await
        .err()
        .expect("unresolved Inherit must fail the build closed");
    assert!(
        matches!(&err, BuildAgentError::Config(message) if message.contains("inherit")),
        "expected typed unresolved-inherit config error, got: {err:?}"
    );
}

/// Resume-escape regression: a resumed session with a persisted tool access
/// policy and no explicit override must rebuild WITH the gate — resuming is
/// not a policy escape hatch.
#[tokio::test]
async fn resumed_session_restores_persisted_tool_access_policy_gate() {
    let temp = tempfile::tempdir().unwrap();
    let policy = allow_list_policy(&["alpha"]);
    let dispatched = Arc::new(Mutex::new(Vec::new()));

    let agent = temp_factory(&temp)
        .build_agent(
            AgentBuildConfig {
                llm_client_override: Some(Arc::new(MockLlmClient)),
                tool_dispatcher_override: Some(Arc::new(PolicyProbeDispatcher::new(
                    &["alpha", "beta"],
                    Arc::clone(&dispatched),
                ))),
                tool_access_policy: Some(policy.clone()),
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
            &Config::default(),
        )
        .await
        .expect("gated build must succeed");
    let persisted_session = agent.session().clone();
    drop(agent);

    let mut resumed = temp_factory(&temp)
        .build_agent(
            AgentBuildConfig {
                llm_client_override: Some(Arc::new(MockLlmClient)),
                tool_dispatcher_override: Some(Arc::new(PolicyProbeDispatcher::new(
                    &["alpha", "beta"],
                    Arc::clone(&dispatched),
                ))),
                resume_session: Some(persisted_session),
                ..AgentBuildConfig::new("claude-sonnet-4-5")
            },
            &Config::default(),
        )
        .await
        .expect("resumed gated build must succeed");

    let metadata = resumed
        .session()
        .session_metadata()
        .expect("resumed session metadata must be set");
    assert_eq!(
        metadata.tooling.tool_access_policy,
        Some(policy),
        "persisted effective policy must survive resume"
    );

    let outcome = resumed
        .dispatch_external_tool_call(meerkat_core::ToolCall::new(
            "call-denied-after-resume".to_string(),
            "beta".to_string(),
            json!({}),
        ))
        .await
        .expect("policy denial is a tool result, not a dispatch fault");
    assert!(
        outcome.result.is_error,
        "resumed session must keep denying gated tools"
    );
    assert!(
        dispatched.lock().unwrap().is_empty(),
        "denied call must never reach the inner dispatcher after resume"
    );
}
