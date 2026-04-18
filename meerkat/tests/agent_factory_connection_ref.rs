//! T3 top-down integration test for Phase 3 of the provider-auth redesign.
//!
//! Exercises the full factory-side dispatch: `AgentBuildConfig.connection_ref`
//! → `ProviderRuntimeRegistry::resolve` → `build_client` → live `DynAgent`
//! with the expected provider identity. Plus the resume round-trip over
//! `SessionMetadata.connection_ref` and the coexistence properties
//! (llm_client_override beats connection_ref; absent connection_ref falls
//! through to the legacy flat path unchanged).
//!
//! This is the RCT Mini top-down test that drives Phase 3 implementation.
//! It is written to fail at assertion, not at panic: unimplemented paths
//! return typed `BuildAgentError::ConnectionResolution(_)` errors rather
//! than `todo!()`/panic, so the test reaches its final assertion and
//! fails with a useful diff.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use meerkat::{AgentBuildConfig, AgentFactory, BuildAgentError};
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmEvent, LlmRequest};
use meerkat_core::{
    AuthProfileConfig, BackendProfileConfig, Config, ConnectionRef, CredentialSourceSpec, Provider,
    ProviderBindingConfig, RealmConfigSection,
};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------

fn temp_factory(temp: &tempfile::TempDir) -> AgentFactory {
    AgentFactory::new(temp.path().join("sessions"))
}

/// Build a Config whose `realm.dev` has one InlineSecret binding per provider,
/// pointing at the matching backend_kind. InlineSecret carries a fake key —
/// the provider clients never hit the network in these tests (we only
/// assert `.provider()` identity, never `.stream()`).
fn config_with_realm() -> Config {
    let mut backend = BTreeMap::new();
    backend.insert(
        "openai_api".to_string(),
        BackendProfileConfig {
            provider: "openai".into(),
            backend_kind: "openai_api".into(),
            base_url: None,
            options: serde_json::Value::Null,
        },
    );
    backend.insert(
        "anthropic_api".to_string(),
        BackendProfileConfig {
            provider: "anthropic".into(),
            backend_kind: "anthropic_api".into(),
            base_url: None,
            options: serde_json::Value::Null,
        },
    );
    backend.insert(
        "google_genai".to_string(),
        BackendProfileConfig {
            provider: "gemini".into(),
            backend_kind: "google_genai".into(),
            base_url: None,
            options: serde_json::Value::Null,
        },
    );

    let mut auth = BTreeMap::new();
    for (id, provider, secret) in [
        ("openai_key", "openai", "sk-openai-test"),
        ("anthropic_key", "anthropic", "sk-anthropic-test"),
        ("google_key", "gemini", "sk-google-test"),
    ] {
        auth.insert(
            id.to_string(),
            AuthProfileConfig {
                provider: provider.into(),
                auth_method: "api_key".into(),
                source: CredentialSourceSpec::InlineSecret {
                    secret: secret.into(),
                },
                storage: None,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
    }

    let mut binding = BTreeMap::new();
    for (id, backend_id, auth_id) in [
        ("default_openai", "openai_api", "openai_key"),
        ("default_anthropic", "anthropic_api", "anthropic_key"),
        ("default_google", "google_genai", "google_key"),
    ] {
        binding.insert(
            id.to_string(),
            ProviderBindingConfig {
                backend_profile: backend_id.into(),
                auth_profile: auth_id.into(),
                default_model: None,
                policy: Default::default(),
            },
        );
    }

    let section = RealmConfigSection {
        backend,
        auth,
        binding,
        default_binding: Some("default_openai".into()),
    };

    let mut config = Config::default();
    config.realm.insert("dev".into(), section);
    config
}

fn conn_ref(binding: &str) -> ConnectionRef {
    ConnectionRef {
        realm_id: "dev".into(),
        binding_id: binding.into(),
    }
}

// ---------------------------------------------------------------------
// K4: factory binding resolution (happy path per provider)
// ---------------------------------------------------------------------

/// Minimal mock client for the `llm_client_override` test (below).
struct MockLlmClient;

#[async_trait]
impl LlmClient for MockLlmClient {
    fn stream<'a>(
        &'a self,
        _request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, meerkat_client::LlmError>> + Send + 'a>>
    {
        Box::pin(stream::iter(vec![Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Success {
                stop_reason: meerkat_core::StopReason::EndTurn,
            },
        })]))
    }

    fn provider(&self) -> &'static str {
        "mock"
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[tokio::test]
async fn build_agent_with_openai_connection_ref_resolves_through_registry() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gpt-5.2");
    build.connection_ref = Some(conn_ref("default_openai"));

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("connection_ref resolves through ProviderRuntimeRegistry");
    // C5 observable: SessionMetadata.provider identity matches the binding's backend provider.
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session metadata written");
    assert_eq!(metadata.provider, Provider::OpenAI);
}

#[tokio::test]
async fn build_agent_with_anthropic_connection_ref_resolves_through_registry() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("claude-sonnet-4-6");
    build.connection_ref = Some(conn_ref("default_anthropic"));

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("connection_ref resolves through ProviderRuntimeRegistry");
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session metadata written");
    assert_eq!(metadata.provider, Provider::Anthropic);
}

#[tokio::test]
async fn build_agent_with_google_connection_ref_resolves_through_registry() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gemini-3.1-pro-preview");
    build.connection_ref = Some(conn_ref("default_google"));

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("connection_ref resolves through ProviderRuntimeRegistry");
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session metadata written");
    assert_eq!(metadata.provider, Provider::Gemini);
}

// ---------------------------------------------------------------------
// Negative: unknown binding → typed ConnectionResolution error (not a
// silent fall-through to the flat path)
// ---------------------------------------------------------------------

#[tokio::test]
async fn build_agent_unknown_binding_surfaces_connection_resolution_error() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gpt-5.2");
    build.connection_ref = Some(conn_ref("does_not_exist"));

    let result = factory.build_agent(build, &config).await;
    match result {
        Err(BuildAgentError::ConnectionResolution(_)) => {}
        Err(other) => panic!("expected ConnectionResolution, got {other:?}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

#[tokio::test]
async fn build_agent_unknown_realm_surfaces_connection_resolution_error() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    // Config has no realm at all → unknown realm path.
    let config = Config::default();

    let mut build = AgentBuildConfig::new("gpt-5.2");
    build.connection_ref = Some(conn_ref("default_openai"));

    let result = factory.build_agent(build, &config).await;
    match result {
        Err(BuildAgentError::ConnectionResolution(_)) => {}
        Err(other) => panic!("expected ConnectionResolution for unknown realm, got {other:?}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

// ---------------------------------------------------------------------
// Precedence: llm_client_override beats connection_ref
// ---------------------------------------------------------------------

#[tokio::test]
async fn llm_client_override_beats_connection_ref() {
    // Observable proof: connection_ref points to an invalid binding
    // ("does_not_exist"). Without override, this returns
    // ConnectionResolution. With override set, the build succeeds —
    // proving the override path bypasses the registry dispatch entirely.
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gpt-5.2");
    build.connection_ref = Some(conn_ref("does_not_exist"));
    build.llm_client_override = Some(Arc::new(MockLlmClient));

    let _agent = factory
        .build_agent(build, &config)
        .await
        .expect("llm_client_override bypasses connection_ref resolution even for invalid bindings");
}

// ---------------------------------------------------------------------
// Coexistence: absent connection_ref still runs the legacy flat path
// ---------------------------------------------------------------------

#[tokio::test]
async fn build_agent_without_connection_ref_uses_flat_path() {
    // Plan §6.9 deleted the legacy per-provider config block. The flat path now relies on the
    // `[realm.default]` inline-secret binding or env vars. This
    // test asserts the shared-map path: no connection_ref, api_key
    // resolved from the map, agent builds successfully.
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let mut config = config_with_realm();
    let section = meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", "flat-key")]);
    config.realm.insert("default".to_string(), section);

    let build = AgentBuildConfig::new("gpt-5.2");
    // No connection_ref — flat path must run unchanged.
    assert!(build.connection_ref.is_none());

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("flat path builds client when api_key is configured");
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session metadata written");
    assert_eq!(metadata.provider, Provider::OpenAI);
}

// ---------------------------------------------------------------------
// C4: SessionMetadata persists connection_ref
// ---------------------------------------------------------------------

#[tokio::test]
async fn session_metadata_persists_connection_ref_across_build() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gpt-5.2");
    build.connection_ref = Some(conn_ref("default_openai"));

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("build_agent with connection_ref");

    let metadata = agent
        .session()
        .session_metadata()
        .expect("SessionMetadata should be populated after build_agent");

    assert_eq!(
        metadata.connection_ref.as_ref(),
        Some(&conn_ref("default_openai")),
        "persisted SessionMetadata must round-trip the connection_ref so resume re-resolves the same binding",
    );
}
