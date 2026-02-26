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
use meerkat_client::LlmClient;
use meerkat_core::{
    Config, Provider, ProviderConfig, Session, SessionId, SessionMetadata, SessionTooling,
    UserMessage,
};
use meerkat_store::{SessionFilter, SessionStore, StoreError};

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

// ---------------------------------------------------------------------------
// Helper: create a factory pointing at a temp directory
// ---------------------------------------------------------------------------

fn temp_factory(temp: &tempfile::TempDir) -> AgentFactory {
    AgentFactory::new(temp.path().join("sessions"))
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

    let result = agent.run("Hello".to_string()).await.unwrap();
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
    assert!(metadata.tooling.builtins);
    assert!(metadata.tooling.shell);
    assert!(!metadata.tooling.comms);
    assert!(!metadata.tooling.subagents);
    // When skills feature is enabled, active_skills should be populated
    #[cfg(feature = "skills")]
    assert!(metadata.tooling.active_skills.is_some());
    #[cfg(not(feature = "skills"))]
    assert!(metadata.tooling.active_skills.is_none());
    assert!(!metadata.host_mode);
    assert!(metadata.comms_name.is_none());
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
    assert!(metadata.tooling.builtins, "builtins should be enabled");
}

/// 6. `build_agent` with `resume_session` preserves existing session messages.
#[tokio::test]
async fn build_agent_with_resume_preserves_messages() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    // Create a session with existing messages
    let mut session = Session::new();
    session.push(meerkat_core::Message::User(UserMessage {
        content: "Previous question".to_string(),
    }));

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
            u.content == "Previous question"
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
        provider: Provider::Anthropic,
        tooling: SessionTooling {
            builtins: true,
            shell: false,
            comms: false,
            subagents: false,
            mob: false,
            active_skills: None,
        },
        host_mode: false,
        comms_name: None,
        peer_meta: None,
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
        max_tokens: Some(4096),
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let agent = factory.build_agent(build_config, &config).await.unwrap();

    let metadata = agent
        .session()
        .session_metadata()
        .expect("session should have metadata");
    assert_eq!(metadata.model, "claude-sonnet-4-5");
    assert_eq!(metadata.max_tokens, 4096);
    assert_eq!(metadata.provider, Provider::Anthropic);
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

/// 9. `build_agent` with host_mode but no comms_name fails.
#[cfg(feature = "comms")]
#[tokio::test]
async fn build_agent_host_mode_without_comms_name_fails() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = Config::default();

    let build_config = AgentBuildConfig {
        llm_client_override: Some(Arc::new(MockLlmClient)),
        host_mode: true,
        comms_name: None,
        ..AgentBuildConfig::new("claude-sonnet-4-5")
    };

    let result = factory.build_agent(build_config, &config).await;
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("Expected HostModeRequiresCommsName error"),
    };
    assert!(
        matches!(err, BuildAgentError::HostModeRequiresCommsName),
        "Should be HostModeRequiresCommsName, got: {err:?}"
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
    async fn save(&self, _session: &Session) -> Result<(), StoreError> {
        self.save_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn load(&self, _id: &SessionId) -> Result<Option<Session>, StoreError> {
        Ok(None)
    }

    async fn list(
        &self,
        _filter: SessionFilter,
    ) -> Result<Vec<meerkat_core::SessionMeta>, StoreError> {
        Ok(vec![])
    }

    async fn delete(&self, _id: &SessionId) -> Result<(), StoreError> {
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
    let result = agent.run("Hello".to_string()).await.unwrap();
    assert!(result.text.contains("Hello from mock"));

    // The custom store should have received at least one save call
    assert!(
        store.save_count() > 0,
        "Custom store should have been used for saving, got {} saves",
        store.save_count()
    );
}
