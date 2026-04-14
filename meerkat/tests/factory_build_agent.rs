//! Integration tests for `AgentFactory::build_agent()`.
//!
//! These tests validate the consolidated agent construction pipeline without
//! requiring live API keys, using mock LLM clients injected via
//! `AgentBuildConfig::llm_client_override`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use meerkat::BuildAgentError;
use meerkat::{AgentBuildConfig, AgentFactory, LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_client::{LlmClient, TestClient};
#[cfg(feature = "comms")]
use meerkat_comms::{CommsRuntime, ResolvedCommsConfig, TrustedPeer, identity::Keypair};
use meerkat_core::service::{MobToolAuthorityContext, OpaquePrincipalToken};
use meerkat_core::{
    AgentToolDispatcher, Config, Provider, ProviderConfig, SelfHostedApiStyle,
    SelfHostedModelConfig, SelfHostedServerConfig, SelfHostedTransport, Session, SessionId,
    SessionLlmIdentity, SessionMetadata, SessionTooling, ToolCallView, ToolCategoryOverride,
    ToolDef, ToolDispatchOutcome, ToolError, UserMessage,
};
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

    fn provider(&self) -> &'static str {
        "mock"
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[derive(Default)]
struct CaptureClient {
    inner: TestClient,
    seen_tools: Mutex<Vec<String>>,
}

impl CaptureClient {
    fn tool_names(&self) -> Vec<String> {
        self.seen_tools.lock().expect("capture lock").clone()
    }
}

#[async_trait]
impl LlmClient for CaptureClient {
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>>
    {
        *self.seen_tools.lock().expect("capture lock") =
            request.tools.iter().map(|tool| tool.name.clone()).collect();
        self.inner.stream(request)
    }

    fn provider(&self) -> &'static str {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        self.inner.health_check().await
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
                name: name.to_string(),
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

fn create_test_authority() -> MobToolAuthorityContext {
    MobToolAuthorityContext::new(OpaquePrincipalToken::new("test-principal"), true)
        .with_managed_mob_scope(["mob-a"])
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

/// 2. `build_agent` without LLM override fails when no API key is set.
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

/// 2b. Provider API key from config.provider is honored when env vars are absent.
#[tokio::test]
async fn build_agent_uses_provider_config_api_key() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config {
        provider: ProviderConfig::OpenAI {
            api_key: Some("test-openai-key".to_string()),
            base_url: None,
        },
        ..Config::default()
    };

    let build_config = AgentBuildConfig::new("gpt-5.2");
    let result = factory.build_agent(build_config, &config).await;
    assert!(
        result.is_ok(),
        "build_agent should accept API key from config.provider: {:?}",
        result.err()
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
            bearer_token: None,
            bearer_token_env: None,
        },
    );
    config.self_hosted.models.insert(
        "gemma-4-31b".to_string(),
        SelfHostedModelConfig {
            server: "local".to_string(),
            remote_model: "gemma4:31b".to_string(),
            display_name: "Gemma 4 31B".to_string(),
            family: "gemma-4".to_string(),
            tier: meerkat_models::ModelTier::Supported,
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
            bearer_token: None,
            bearer_token_env: None,
        },
    );
    config.self_hosted.models.insert(
        "gemma-4-e2b".to_string(),
        SelfHostedModelConfig {
            server: "local".to_string(),
            remote_model: "gemma4:e2b".to_string(),
            display_name: "Gemma 4 E2B".to_string(),
            family: "gemma-4".to_string(),
            tier: meerkat_models::ModelTier::Supported,
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
    let tool_names = run_and_capture_tool_names(
        factory,
        AgentBuildConfig {
            schedule_tools: Some(Arc::new(ScheduleToolDispatcher::new(ScheduleService::new(
                Arc::new(MemoryScheduleStore::default()),
            )))),
            ..AgentBuildConfig::new("claude-sonnet-4-5")
        },
    )
    .await;

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
}

#[tokio::test]
async fn build_agent_without_scheduler_keeps_injected_scheduler_tools_hidden() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).schedule(false);
    let tool_names = run_and_capture_tool_names(
        factory,
        AgentBuildConfig {
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
        name: "test-scheduler-comms".to_string(),
        inproc_namespace: None,
        listen_tcp: None,
        listen_uds: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        identity_dir: temp.path().join("identity"),
        trusted_peers_path: temp.path().join("trusted_peers.json"),
        comms_config: Default::default(),
        auth: Default::default(),
        require_peer_auth: false,
        allow_external_unauthenticated: false,
    };
    let comms_runtime = Arc::new(
        CommsRuntime::new_with_silent_intents(comms_config, Arc::new(Default::default()))
            .await
            .unwrap(),
    );
    comms_runtime
        .register_trusted_peer(TrustedPeer {
            name: "peer-a".into(),
            pubkey: Keypair::generate().public_key(),
            addr: "tcp://127.0.0.1:9999".into(),
            meta: Default::default(),
        })
        .await
        .unwrap();
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
        model: "claude-sonnet-4-5".to_string(),
        max_tokens: 4096,
        structured_output_retries: 2,
        provider: Provider::Anthropic,
        provider_params: Some(json!({
            "reasoning": { "budget_tokens": 2048 }
        })),
        self_hosted_server_id: None,
        tooling: SessionTooling {
            builtins: ToolCategoryOverride::Enable,
            shell: ToolCategoryOverride::Disable,
            comms: ToolCategoryOverride::Disable,
            mob: ToolCategoryOverride::Disable,
            memory: ToolCategoryOverride::Disable,
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
    };
    session.set_session_metadata(original_metadata).unwrap();

    // Resume with the same model (simulating the normal resume flow)
    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        provider: Some(Provider::OpenAI),
        max_tokens: Some(1024),
        provider_params: Some(json!({ "temperature": 0.1 })),
        ..AgentBuildConfig::new("gpt-5.2")
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
        Some(json!({
            "reasoning": { "budget_tokens": 2048 }
        }))
    );
    assert_eq!(
        metadata.comms_name.as_deref(),
        Some("persisted-resume-name"),
        "resumed durable comms identity should win over current build defaults"
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

#[tokio::test]
async fn build_agent_with_resume_preserves_explicit_override_masked_fields() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: Provider::Anthropic,
            provider_params: Some(json!({
                "reasoning": { "budget_tokens": 2048 }
            })),
            self_hosted_server_id: None,
            tooling: SessionTooling {
                builtins: ToolCategoryOverride::Disable,
                shell: ToolCategoryOverride::Disable,
                comms: ToolCategoryOverride::Disable,
                mob: ToolCategoryOverride::Disable,
                memory: ToolCategoryOverride::Disable,
                active_skills: None,
            },
            keep_alive: true,
            comms_name: Some("persisted-name".to_string()),
            peer_meta: Some(meerkat_core::PeerMeta::default().with_label("role", "persisted")),
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
        })
        .unwrap();

    let mut build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        provider: Some(Provider::OpenAI),
        max_tokens: Some(1024),
        provider_params: Some(json!({ "temperature": 0.1 })),
        keep_alive: false,
        comms_name: Some("explicit-name".to_string()),
        peer_meta: Some(meerkat_core::PeerMeta::default().with_label("role", "explicit")),
        ..AgentBuildConfig::new("gpt-5.2")
    };
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

    assert_eq!(metadata.model, "claude-sonnet-4-5");
    assert_eq!(metadata.provider, Provider::OpenAI);
    assert_eq!(metadata.max_tokens, 1024);
    assert_eq!(
        metadata.provider_params,
        Some(json!({ "temperature": 0.1 }))
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
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
        })
        .unwrap();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        ..AgentBuildConfig::new("gpt-5.2")
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
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
        })
        .unwrap();

    let mut build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        resume_session: Some(session),
        override_builtins: ToolCategoryOverride::Inherit,
        ..AgentBuildConfig::new("gpt-5.2")
    };
    build_config.resume_override_mask.override_builtins = true;

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");

    assert_eq!(metadata.tooling.builtins, ToolCategoryOverride::Inherit);
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
                active_skills: None,
            },
            keep_alive: false,
            comms_name: Some("resume-peer".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
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
                active_skills: None,
            },
            keep_alive: false,
            comms_name: Some("resume-peer".to_string()),
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
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
        system_prompt: Some(custom_prompt.clone()),
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
    let expected_max_tokens = config.max_tokens;

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
        preload_skills: Some(vec![meerkat_core::skills::SkillId(
            "nonexistent/skill".into(),
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
        preload_skills: Some(vec![meerkat_core::skills::SkillId("valid-skill".into())]),
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
        Some(vec![meerkat_core::skills::SkillId("valid-skill".into())]),
        "session metadata should persist only the explicitly preloaded skills"
    );

    let invalid_build = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        preload_skills: Some(vec![meerkat_core::skills::SkillId("broken-skill".into())]),
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
                active_skills: Some(vec![meerkat_core::skills::SkillId(
                    "nonexistent-legacy-skill".into(),
                )]),
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
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
        Some(vec![meerkat_core::skills::SkillId(
            "nonexistent-legacy-skill".into(),
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
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
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
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
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

/// Regression: a session with mob=Enable should remain enabled even if
/// the factory default changes to disabled.
#[tokio::test]
async fn resume_with_enable_mob_stays_enabled() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp).mob(false); // factory disables mob
    let config = Config::default();

    let mut session = Session::new();
    session
        .set_session_metadata(SessionMetadata {
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
                mob: ToolCategoryOverride::Enable, // <-- explicitly on
                memory: ToolCategoryOverride::Disable,
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
        })
        .unwrap();

    let mut build_config = AgentBuildConfig::new("claude-sonnet-4-5".to_string());
    build_config.resume_session = Some(session);
    build_config.llm_client_override = Some(Arc::new(MockLlmClient));

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    let metadata = agent.session().session_metadata().unwrap();

    assert_eq!(
        metadata.tooling.mob,
        ToolCategoryOverride::Enable,
        "Enable mob should be preserved on resume despite factory disabling it"
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
    assert_eq!(observed.as_slice(), &[None]);
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
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
        })
        .unwrap();

    let build_config = AgentBuildConfig {
        resume_session: Some(session),
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert_eq!(observed.as_slice(), &[None]);
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
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
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
    build_config.mob_tool_authority_context = Some(create_test_authority());

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert_eq!(observed.as_slice(), &[Some(create_test_authority())]);
}

#[tokio::test]
async fn explicit_mob_authority_is_persisted_on_session() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let mut build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };
    build_config.mob_tool_authority_context = Some(create_test_authority());

    let agent = factory.build_agent(build_config, &config).await.unwrap();
    assert_eq!(
        agent.session().mob_tool_authority_context(),
        Some(create_test_authority())
    );
}

#[tokio::test]
async fn resumed_persisted_mob_authority_is_forwarded_to_mob_tools_factory() {
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
                active_skills: None,
            },
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
        })
        .unwrap();
    session
        .set_mob_tool_authority_context(Some(create_test_authority()))
        .unwrap();

    let build_config = AgentBuildConfig {
        resume_session: Some(session),
        llm_client_override: Some(Arc::new(MockLlmClient)),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let _agent = factory.build_agent(build_config, &config).await.unwrap();
    let observed = observed.lock().expect("recording mutex");
    assert_eq!(observed.as_slice(), &[Some(create_test_authority())]);
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
        name: "shared-parent".to_string(),
        inproc_namespace: None,
        listen_tcp: None,
        listen_uds: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        identity_dir: temp.path().join("identity"),
        trusted_peers_path: temp.path().join("trusted_peers.json"),
        comms_config: Default::default(),
        auth: Default::default(),
        require_peer_auth: false,
        allow_external_unauthenticated: false,
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
    shared_runtime
        .register_trusted_peer(meerkat_comms::TrustedPeer {
            name: "sentinel".into(),
            pubkey: meerkat_comms::identity::Keypair::generate().public_key(),
            addr: "tcp://127.0.0.1:9999".into(),
            meta: meerkat_comms::PeerMeta::default(),
        })
        .await
        .unwrap();

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
    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>>
    {
        *self.captured.lock().unwrap() = request
            .provider_params
            .clone()
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
    fn provider(&self) -> &'static str {
        "anthropic"
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
        provider_params: Some(json!({"web_search": null})),
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
        provider_params: Some(json!({"thinking_budget": 5000})),
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
        params.get("thinking_budget").is_some(),
        "explicit thinking_budget should be present: {params}"
    );
}
