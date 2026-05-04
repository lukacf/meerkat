//! T3 top-down integration test for Phase 3 of the provider-auth redesign.
//!
//! Exercises the full factory-side dispatch: `AgentBuildConfig.auth_binding`
//! → `ProviderRuntimeRegistry::resolve` → `build_client` → live `DynAgent`
//! with the expected provider identity. Plus the resume round-trip over
//! `SessionMetadata.auth_binding` and the coexistence properties
//! (llm_client_override beats auth_binding; absent auth_binding falls
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
    AuthBindingRef, AuthProfileConfig, BackendProfileConfig, Config, CredentialSourceSpec,
    Provider, ProviderBindingConfig, RealmConfigSection,
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

fn conn_ref(binding: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
        binding: meerkat_core::BindingId::parse(binding).expect("valid binding"),
        profile: None,
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
async fn build_agent_with_openai_auth_binding_resolves_through_registry() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gpt-5.4");
    build.auth_binding = Some(conn_ref("default_openai"));

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("auth_binding resolves through ProviderRuntimeRegistry");
    // C5 observable: SessionMetadata.provider identity matches the binding's backend provider.
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session metadata written");
    assert_eq!(metadata.provider, Provider::OpenAI);
}

#[tokio::test]
async fn build_agent_with_anthropic_auth_binding_resolves_through_registry() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("claude-sonnet-4-6");
    build.auth_binding = Some(conn_ref("default_anthropic"));

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("auth_binding resolves through ProviderRuntimeRegistry");
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session metadata written");
    assert_eq!(metadata.provider, Provider::Anthropic);
}

#[tokio::test]
async fn build_agent_with_google_auth_binding_resolves_through_registry() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gemini-3.1-pro-preview");
    build.auth_binding = Some(conn_ref("default_google"));

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("auth_binding resolves through ProviderRuntimeRegistry");
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session metadata written");
    assert_eq!(metadata.provider, Provider::Gemini);
}

// ---------------------------------------------------------------------
// Capability-driven routing: realtime-capable OpenAI models must go
// through `OpenAiRealtimeTextAdapter`, not the Responses-API client.
// The pair below proves the two legs of the `cfg(feature)` gate: with
// the feature on, `build_agent` succeeds (only the realtime branch
// can produce an `Arc<dyn LlmClient>` here, since `realtime_route` is
// true for this model); with the feature off, the same build returns
// a typed `ConnectionResolution` naming the missing feature.
// ---------------------------------------------------------------------

#[cfg(feature = "openai-realtime")]
#[tokio::test]
async fn build_agent_with_gpt_realtime_selects_realtime_text_adapter() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gpt-realtime-1.5");
    build.auth_binding = Some(conn_ref("default_openai"));

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("gpt-realtime-1.5 routes through OpenAiRealtimeTextAdapter");
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session metadata written");
    assert_eq!(metadata.provider, Provider::OpenAI);
}

#[cfg(not(feature = "openai-realtime"))]
#[tokio::test]
async fn build_agent_with_gpt_realtime_without_feature_returns_typed_error() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gpt-realtime-1.5");
    build.auth_binding = Some(conn_ref("default_openai"));

    let result = factory.build_agent(build, &config).await;
    match result {
        Err(BuildAgentError::ConnectionResolution(msg)) => assert!(
            msg.contains("openai-realtime") && msg.contains("feature"),
            "error must name the missing feature; got: {msg}"
        ),
        Err(other) => panic!("expected ConnectionResolution, got {other:?}"),
        Ok(_) => panic!("expected error when openai-realtime feature is absent"),
    }
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

    let mut build = AgentBuildConfig::new("gpt-5.4");
    build.auth_binding = Some(conn_ref("does_not_exist"));

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

    let mut build = AgentBuildConfig::new("gpt-5.4");
    build.auth_binding = Some(conn_ref("default_openai"));

    let result = factory.build_agent(build, &config).await;
    match result {
        Err(BuildAgentError::ConnectionResolution(_)) => {}
        Err(other) => panic!("expected ConnectionResolution for unknown realm, got {other:?}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

// ---------------------------------------------------------------------
// Precedence: llm_client_override beats auth_binding
// ---------------------------------------------------------------------

#[tokio::test]
async fn llm_client_override_beats_auth_binding() {
    // Observable proof: auth_binding points to an invalid binding
    // ("does_not_exist"). Without override, this returns
    // ConnectionResolution. With override set, the build succeeds —
    // proving the override path bypasses the registry dispatch entirely.
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gpt-5.4");
    build.auth_binding = Some(conn_ref("does_not_exist"));
    build.llm_client_override = Some(Arc::new(MockLlmClient));

    let _agent = factory
        .build_agent(build, &config)
        .await
        .expect("llm_client_override bypasses auth_binding resolution even for invalid bindings");
}

// ---------------------------------------------------------------------
// Coexistence: absent auth_binding still resolves the configured default
// ---------------------------------------------------------------------

#[tokio::test]
async fn build_agent_without_auth_binding_uses_default_realm_binding() {
    // A missing auth_binding is not an ambient-credential bypass. It resolves
    // through the same typed realm binding machinery as explicit auth_binding
    // builds, choosing config.realm["default"].default_binding when present and
    // persisting the resolved ref into SessionMetadata.
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let mut config = config_with_realm();
    let section = meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", "flat-key")]);
    config.realm.insert("default".to_string(), section);

    let build = AgentBuildConfig::new("gpt-5.4");
    assert!(build.auth_binding.is_none());

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("default realm binding should resolve without explicit auth_binding");
    let metadata = agent
        .session()
        .session_metadata()
        .expect("session metadata written");
    assert_eq!(metadata.provider, Provider::OpenAI);
    assert_eq!(
        metadata.auth_binding.as_ref().map(|conn_ref| {
            (
                conn_ref.realm.as_str().to_string(),
                conn_ref.binding.as_str().to_string(),
            )
        }),
        Some(("default".to_string(), "default_openai".to_string()))
    );
}

// ---------------------------------------------------------------------
// C4: SessionMetadata persists auth_binding
// ---------------------------------------------------------------------

#[tokio::test]
async fn session_metadata_persists_auth_binding_across_build() {
    let temp = tempfile::tempdir().unwrap();
    let factory = temp_factory(&temp);
    let config = config_with_realm();

    let mut build = AgentBuildConfig::new("gpt-5.4");
    build.auth_binding = Some(conn_ref("default_openai"));

    let agent = factory
        .build_agent(build, &config)
        .await
        .expect("build_agent with auth_binding");

    let metadata = agent
        .session()
        .session_metadata()
        .expect("SessionMetadata should be populated after build_agent");

    assert_eq!(
        metadata.auth_binding.as_ref(),
        Some(&conn_ref("default_openai")),
        "persisted SessionMetadata must round-trip the auth_binding so resume re-resolves the same binding",
    );
}
