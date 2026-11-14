//! A session's `reasoning_effort` level is checked against the route of the
//! client its build made.
//!
//! The level a build's params carry is resolved, strictly, for the model and
//! the route the built client observes, next to the other build-time request
//! shaping checks: at a fresh build, at a resumed build (which keeps the
//! durable level through the resume merge), at the build that recovers a
//! session that is not live, and when a live session is reconfigured. A level
//! the route does not accept is refused typed; nothing is published, no
//! provider request is made, and the stored identity stays as it was.
//!
//! The provider layer is a credential-free fake `ProviderRuntime` per
//! provider whose client reports a scripted route observation (the provider
//! family's direct route unless a row scripts another), so binding selection,
//! catalog validation and client construction are the factory's real path.
//! That is why this lives under `factory`: the registry slot is private.

use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::lifecycle::run_primitive::ProviderParamsOverride;
use meerkat_core::model_profile::capabilities::EffortLevel;
use meerkat_core::provider_matrix::OpenAiBackendKind;
use meerkat_core::{
    AccountLevelStatement, AuthBindingRef, BackendProfileConfig, BindingId, Config,
    CredentialSourceSpec, ModelApi, Provider, ProviderBindingConfig, RealmConfigSection, RealmId,
    ReasoningBackend, ReasoningEffortRefusal, ReasoningRouteObservation, Session,
    SessionLlmIdentity, StopReason, ToolCategoryOverride,
};
use meerkat_llm_core::provider_runtime::{
    ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
    ResolvedConnection, ResolvedTextTarget, ResolverEnvironment, StaticLease, ValidatedBinding,
};
use serde_json::json;

use crate::{AgentBuildConfig, AgentFactory, BuildAgentError, DynAgent};

const REALM: &str = "default";
const ANTHROPIC_BINDING: &str = "default_anthropic";
const OPENAI_BINDING: &str = "default_openai";
/// Its catalog row accepts `low`, `medium`, `high` and `max`.
const WIDE_MODEL: &str = "claude-sonnet-4-6";
/// Its catalog row accepts `low`, `medium` and `high`.
const NARROW_MODEL: &str = "claude-opus-4-5";
const OPENAI_MODEL: &str = "gpt-5.4";
/// A model the catalog does not know, defined by the build.
const UNCATALOGUED_MODEL: &str = "custom-direct-model-1";
const ANSWER: &str = "answered";

fn binding(name: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(REALM).expect("valid realm"),
        binding: BindingId::parse(name).expect("valid binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

fn binding_for(provider: Provider) -> AuthBindingRef {
    binding(match provider {
        Provider::Anthropic => ANTHROPIC_BINDING,
        _ => OPENAI_BINDING,
    })
}

/// One API-key route per provider in the test realm.
fn test_config() -> Config {
    let mut section = RealmConfigSection::default();
    for (name, provider, backend_kind) in [
        (ANTHROPIC_BINDING, Provider::Anthropic, "anthropic_api"),
        (OPENAI_BINDING, Provider::OpenAI, "openai_api"),
    ] {
        section.backend.insert(
            name.to_string(),
            BackendProfileConfig {
                provider: provider.as_str().to_string(),
                backend_kind: backend_kind.to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            },
        );
        section.auth.insert(
            name.to_string(),
            meerkat_core::AuthProfileConfig {
                provider: provider.as_str().to_string(),
                auth_method: "api_key".to_string(),
                source: CredentialSourceSpec::InlineSecret {
                    secret: format!("unused-test-key-{name}"),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            name.to_string(),
            ProviderBindingConfig {
                backend_profile: name.to_string(),
                auth_profile: name.to_string(),
                credential_account: None,
                default_model: None,
                policy: Default::default(),
                provider_default: true,
            },
        );
    }
    let mut config = Config::default();
    config.realm.insert(REALM.to_string(), section);
    config
}

/// What the fake provider layer observed, and the route its clients report.
#[derive(Default)]
struct Script {
    /// Every provider call: the provider that served it and its model.
    served: StdMutex<Vec<(Provider, String)>>,
    /// Scripted route observations, newest last; a provider with none
    /// reports its family's direct route.
    routes: StdMutex<Vec<(Provider, Option<ReasoningRouteObservation>)>>,
}

impl Script {
    fn served(&self) -> Vec<(Provider, String)> {
        self.served
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn clear_served(&self) {
        self.served
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// The route `provider`'s clients report from now on (`None`: no route).
    fn set_route(&self, provider: Provider, observation: Option<ReasoningRouteObservation>) {
        self.routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((provider, observation));
    }

    fn route(&self, provider: Provider) -> Option<ReasoningRouteObservation> {
        self.routes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .rev()
            .find(|(scripted, _)| *scripted == provider)
            .map(|(_, observation)| observation.clone())
            .unwrap_or_else(|| meerkat_core::direct_route_observation(provider))
    }
}

/// Answers every request with one text turn and reports the scripted route.
struct RouteClient {
    provider: Provider,
    script: Arc<Script>,
}

#[async_trait]
impl LlmClient for RouteClient {
    fn reasoning_route(&self) -> Option<ReasoningRouteObservation> {
        self.script.route(self.provider)
    }

    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> std::pin::Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        self.script
            .served
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((self.provider, request.model.clone()));
        Box::pin(futures::stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: ANSWER.to_string(),
                meta: None,
            }),
            Ok(LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    self.provider,
                    &request.model,
                    meerkat_core::Usage::default(),
                ),
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            }),
        ]))
    }

    fn provider(&self) -> Provider {
        self.provider
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Credential-free provider runtime: resolves the validated binding to a
/// static non-secret lease and builds the scripted client.
struct RouteRuntime {
    provider: Provider,
    script: Arc<Script>,
}

impl RouteRuntime {
    fn client(&self) -> Arc<dyn LlmClient> {
        Arc::new(RouteClient {
            provider: self.provider,
            script: Arc::clone(&self.script),
        })
    }
}

#[async_trait]
impl ProviderRuntime for RouteRuntime {
    fn provider_id(&self) -> Provider {
        self.provider
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        _env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        Ok(ResolvedConnection {
            provider: self.provider,
            backend: binding.backend(),
            backend_profile: Arc::clone(binding.backend_profile()),
            credential_identity: binding.credential_identity().clone(),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "unused-test-key".to_string(),
                meerkat_core::AuthMetadata::default(),
                None,
                format!("reasoning-effort-build-test:{:?}", self.provider),
            )),
        })
    }

    fn build_client(
        &self,
        _connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Ok(self.client())
    }

    fn build_text_client(
        &self,
        _target: ResolvedTextTarget,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Ok(self.client())
    }
}

fn scripted_factory(store_path: std::path::PathBuf, script: &Arc<Script>) -> AgentFactory {
    let mut factory = AgentFactory::new(store_path)
        .without_provider_auth_persistence()
        .builtins(false);
    let mut registry = ProviderRuntimeRegistry::empty();
    for provider in [Provider::OpenAI, Provider::Anthropic] {
        registry = registry.with_runtime(Arc::new(RouteRuntime {
            provider,
            script: Arc::clone(script),
        }));
    }
    factory.provider_registry = Arc::new(registry);
    factory
}

struct Harness {
    _temp: tempfile::TempDir,
    store_path: std::path::PathBuf,
    script: Arc<Script>,
    factory: AgentFactory,
    config: Config,
}

fn level(level: EffortLevel) -> Option<ProviderParamsOverride> {
    Some(ProviderParamsOverride {
        reasoning_effort: Some(level),
        ..Default::default()
    })
}

impl Harness {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let store_path = temp.path().join("sessions");
        let script = Arc::new(Script::default());
        let factory = scripted_factory(store_path.clone(), &script);
        Self {
            _temp: temp,
            store_path,
            script,
            factory,
            config: test_config(),
        }
    }

    fn build_config(
        &self,
        model: &str,
        provider: Provider,
        effort: Option<EffortLevel>,
    ) -> AgentBuildConfig {
        let mut build = AgentBuildConfig::new(model);
        build.provider = Some(provider);
        build.realm_id = Some(RealmId::parse(REALM).expect("valid realm"));
        build.auth_binding = Some(binding_for(provider));
        build.override_image_generation = ToolCategoryOverride::Disable;
        build.provider_params = effort.and_then(level);
        build
    }

    async fn build(
        &self,
        model: &str,
        provider: Provider,
        effort: Option<EffortLevel>,
    ) -> Result<DynAgent, BuildAgentError> {
        self.factory
            .build_agent(self.build_config(model, provider, effort), &self.config)
            .await
    }

    /// A durable session of [`WIDE_MODEL`] at `max`, as its build leaves it
    /// (metadata and build state), with no turn run.
    async fn durable_wide_session_at_max(&self) -> Session {
        let agent = self
            .build(WIDE_MODEL, Provider::Anthropic, Some(EffortLevel::Max))
            .await
            .unwrap_or_else(|error| panic!("a level the route accepts builds: {error}"));
        let session = agent.session().clone();
        drop(agent);
        assert_eq!(
            durable_identity(&session)
                .provider_params
                .and_then(|params| params.reasoning_effort),
            Some(EffortLevel::Max),
            "the build stores the level as the neutral field, not rewritten"
        );
        session
    }
}

fn durable_identity(session: &Session) -> SessionLlmIdentity {
    session
        .session_metadata()
        .expect("the session carries its metadata")
        .llm_identity()
}

fn expect_refused(result: Result<DynAgent, BuildAgentError>) -> ReasoningEffortRefusal {
    match result {
        Err(BuildAgentError::ReasoningEffortRefused(refusal)) => *refusal,
        Err(other) => panic!("expected the typed reasoning effort refusal, got: {other}"),
        Ok(_) => panic!("a level the route does not accept must not build"),
    }
}

fn refusal_json(refusal: &ReasoningEffortRefusal) -> serde_json::Value {
    serde_json::to_value(refusal).expect("a refusal serializes")
}

/// The refusal of `max` for [`NARROW_MODEL`] on the direct Anthropic route.
fn narrow_model_refusal_document() -> serde_json::Value {
    json!({
        "requested": "max", "provider": "anthropic", "model": NARROW_MODEL,
        "route": {"backend": "anthropic_api", "api": "messages"},
        "reason": "unsupported_level", "supported": ["low", "medium", "high"],
        "facts": "catalog",
    })
}

fn copilot_responses(account: AccountLevelStatement) -> ReasoningRouteObservation {
    ReasoningRouteObservation {
        backend: ReasoningBackend::OpenAi(OpenAiBackendKind::Copilot),
        api: ModelApi::Responses,
        account,
        operator_levels: None,
    }
}

/// A catalogued level the model does not accept on its direct route is
/// refused when the agent is built: typed, with the catalog's levels, and no
/// provider request is made. The same model builds at a level its row
/// accepts, and a model whose row accepts the level builds with it.
#[tokio::test]
async fn build_refuses_an_unsupported_level_typed() {
    let harness = Harness::new();
    let refusal = expect_refused(
        harness
            .build(NARROW_MODEL, Provider::Anthropic, Some(EffortLevel::Max))
            .await,
    );
    assert_eq!(refusal_json(&refusal), narrow_model_refusal_document());
    assert_eq!(
        BuildAgentError::ReasoningEffortRefused(Box::new(refusal.clone())).to_string(),
        "reasoning effort 'max' is not supported by anthropic model 'claude-opus-4-5' on \
         anthropic_api/messages; supported: low, medium, high"
    );
    assert!(
        harness.script.served().is_empty(),
        "a refused build makes no provider request: {:?}",
        harness.script.served()
    );

    harness
        .build(NARROW_MODEL, Provider::Anthropic, Some(EffortLevel::High))
        .await
        .unwrap_or_else(|error| panic!("a level the row accepts builds: {error}"));
    harness
        .build(WIDE_MODEL, Provider::Anthropic, Some(EffortLevel::Max))
        .await
        .unwrap_or_else(|error| panic!("a model whose row accepts the level builds: {error}"));
    assert!(harness.script.served().is_empty());
}

/// A build that carries no level is not judged at all: it builds on a client
/// that reports no route, exactly as before.
#[tokio::test]
async fn a_build_without_a_level_is_not_judged() {
    let harness = Harness::new();
    harness.script.set_route(Provider::Anthropic, None);
    let mut agent = harness
        .build(NARROW_MODEL, Provider::Anthropic, None)
        .await
        .unwrap_or_else(|error| panic!("a build without a level builds: {error}"));
    let result = agent
        .run("hello".to_string().into())
        .await
        .expect("the turn runs");
    assert_eq!(result.text, ANSWER);
}

/// The session service's builder answers the refused build as the typed
/// session error: invalid params with the documented data, and no session.
#[tokio::test]
async fn a_refused_build_is_the_typed_session_error() {
    use meerkat_session::ephemeral::SessionAgentBuilder as _;

    let temp = tempfile::tempdir().expect("tempdir");
    let script = Arc::new(Script::default());
    let builder = crate::FactoryAgentBuilder::new(
        scripted_factory(temp.path().join("sessions"), &script),
        test_config(),
    );
    let request = meerkat_core::service::CreateSessionRequest {
        injected_context: Vec::new(),
        model: NARROW_MODEL.to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        system_prompt: crate::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(meerkat_core::service::SessionBuildOptions {
            provider: Some(Provider::Anthropic),
            realm_id: Some(RealmId::parse(REALM).expect("valid realm")),
            auth_binding: Some(binding(ANTHROPIC_BINDING)),
            override_image_generation: ToolCategoryOverride::Disable,
            provider_params: level(EffortLevel::Max),
            ..Default::default()
        }),
        labels: None,
    };
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let error = match builder.build_agent(&request, tx).await {
        Ok(_) => panic!("a level the route does not accept must not build"),
        Err(error) => error,
    };
    assert_eq!(
        meerkat_contracts::error::session_transport_error_code(&error),
        meerkat_contracts::ErrorCode::InvalidParams
    );
    assert_eq!(
        error.structured_data(),
        Some(json!({
            "kind": "reasoning_effort_refused",
            "retryable": false,
            "reasoning_effort_refusal": narrow_model_refusal_document(),
        }))
    );
    assert!(script.served().is_empty());
}

/// Support nobody states, or that cannot be read now, is refused typed with
/// its own reason: an uncatalogued model on a direct route and a client that
/// reports no route are unknown; an account list that is not loaded is
/// unavailable; an account entry that states no levels is unknown although
/// the model has a catalog row; and a level the account's list does not
/// state is unsupported with the account's levels, not the catalog's.
#[tokio::test]
async fn build_refuses_unknown_and_unavailable_support_typed() {
    let harness = Harness::new();

    // An uncatalogued model on the direct route.
    let mut build = harness.build_config(
        UNCATALOGUED_MODEL,
        Provider::Anthropic,
        Some(EffortLevel::High),
    );
    build.custom_models.insert(
        UNCATALOGUED_MODEL.to_string(),
        meerkat_core::config::CustomModelConfig {
            provider: Provider::Anthropic,
            display_name: None,
            context_window: Some(200_000),
            max_input_tokens: None,
            max_output_tokens: Some(32_000),
            vision: None,
            web_search: None,
            call_timeout_secs: None,
        },
    );
    let refusal = expect_refused(harness.factory.build_agent(build, &harness.config).await);
    assert_eq!(
        refusal_json(&refusal),
        json!({
            "requested": "high", "provider": "anthropic", "model": UNCATALOGUED_MODEL,
            "route": {"backend": "anthropic_api", "api": "messages"},
            "reason": "supported_levels_unknown", "supported": null, "facts": null,
        })
    );

    // A client that reports no route: unknown, and no route is guessed.
    harness.script.set_route(Provider::Anthropic, None);
    let refusal = expect_refused(
        harness
            .build(WIDE_MODEL, Provider::Anthropic, Some(EffortLevel::High))
            .await,
    );
    assert_eq!(
        refusal_json(&refusal),
        json!({
            "requested": "high", "provider": "anthropic", "model": WIDE_MODEL,
            "route": null,
            "reason": "supported_levels_unknown", "supported": null, "facts": null,
        })
    );

    // An account route whose model list is not loaded: the transient reason.
    harness.script.set_route(
        Provider::OpenAI,
        Some(copilot_responses(AccountLevelStatement::ListUnavailable)),
    );
    let refusal = expect_refused(
        harness
            .build(OPENAI_MODEL, Provider::OpenAI, Some(EffortLevel::High))
            .await,
    );
    assert_eq!(
        refusal_json(&refusal),
        json!({
            "requested": "high", "provider": "openai", "model": OPENAI_MODEL,
            "route": {"backend": "copilot", "api": "responses"},
            "reason": "supported_levels_unavailable", "supported": null, "facts": null,
        })
    );
    assert!(refusal.reason.is_transient());

    // An account entry that states no levels: unknown, although the model
    // has a catalog row that lists the level.
    harness.script.set_route(
        Provider::OpenAI,
        Some(copilot_responses(AccountLevelStatement::NotStated)),
    );
    let refusal = expect_refused(
        harness
            .build(OPENAI_MODEL, Provider::OpenAI, Some(EffortLevel::High))
            .await,
    );
    assert_eq!(
        refusal_json(&refusal),
        json!({
            "requested": "high", "provider": "openai", "model": OPENAI_MODEL,
            "route": {"backend": "copilot", "api": "responses"},
            "reason": "supported_levels_unknown", "supported": null, "facts": null,
        })
    );
    assert!(!refusal.reason.is_transient());

    // The account's list states the levels: a level it does not state is
    // refused with the account's set, and a level it states builds.
    harness.script.set_route(
        Provider::OpenAI,
        Some(copilot_responses(AccountLevelStatement::Stated(vec![
            EffortLevel::Low,
            EffortLevel::Medium,
            EffortLevel::High,
        ]))),
    );
    let refusal = expect_refused(
        harness
            .build(OPENAI_MODEL, Provider::OpenAI, Some(EffortLevel::None))
            .await,
    );
    assert_eq!(
        refusal_json(&refusal),
        json!({
            "requested": "none", "provider": "openai", "model": OPENAI_MODEL,
            "route": {"backend": "copilot", "api": "responses"},
            "reason": "unsupported_level", "supported": ["low", "medium", "high"],
            "facts": "account",
        })
    );
    harness
        .build(OPENAI_MODEL, Provider::OpenAI, Some(EffortLevel::High))
        .await
        .unwrap_or_else(|error| panic!("a level the account's list states builds: {error}"));

    assert!(
        harness.script.served().is_empty(),
        "no refused or accepted build makes a provider request: {:?}",
        harness.script.served()
    );
}

/// A resumed build is checked like a new one. A resume that changes the
/// model keeps the durable level (the resume merge's rule) and is refused
/// typed when the new model's route does not accept it. The refusal writes
/// nothing: the stored identity is untouched, an unchanged resume still
/// builds, and a resume that names an accepted level with the new model
/// builds.
#[cfg(feature = "jsonl-store")]
#[tokio::test]
async fn a_resumed_build_is_checked_like_a_new_one() {
    use crate::SessionStore as _;

    let harness = Harness::new();
    let durable = harness.durable_wide_session_at_max().await;
    let session_id = durable.id().clone();
    let store = meerkat_store::JsonlStore::new(harness.store_path.clone());
    store.init().await.expect("open the session store");
    store.save(&durable).await.expect("store the session");
    let before = durable_identity(&durable);
    assert_eq!(before.model, WIDE_MODEL);

    let mut resume = harness.build_config(NARROW_MODEL, Provider::Anthropic, None);
    resume.resume_override_mask.model = true;
    resume.resume_session = Some(durable.clone());
    let refusal = expect_refused(harness.factory.build_agent(resume, &harness.config).await);
    assert_eq!(refusal_json(&refusal), narrow_model_refusal_document());
    assert!(harness.script.served().is_empty());

    let stored = store
        .load(&session_id)
        .await
        .expect("read the stored session")
        .expect("the session is stored");
    assert_eq!(
        durable_identity(&stored),
        before,
        "a refused resume writes nothing"
    );

    // Durable truth wins on a resume with no override: it still builds.
    let mut unchanged = harness.build_config(WIDE_MODEL, Provider::Anthropic, None);
    unchanged.resume_session = Some(stored.clone());
    let resumed = harness
        .factory
        .build_agent(unchanged, &harness.config)
        .await
        .unwrap_or_else(|error| panic!("a resume without the override still builds: {error}"));
    assert_eq!(durable_identity(resumed.session()), before);
    drop(resumed);

    // The caller names a level the new model accepts in the same resume.
    let mut relevelled =
        harness.build_config(NARROW_MODEL, Provider::Anthropic, Some(EffortLevel::High));
    relevelled.resume_override_mask.model = true;
    relevelled.resume_override_mask.reasoning_effort = true;
    relevelled.resume_session = Some(stored);
    let resumed = harness
        .factory
        .build_agent(relevelled, &harness.config)
        .await
        .unwrap_or_else(|error| panic!("a resume that names an accepted level builds: {error}"));
    let identity = durable_identity(resumed.session());
    assert_eq!(identity.model, NARROW_MODEL);
    assert_eq!(
        identity
            .provider_params
            .and_then(|params| params.reasoning_effort),
        Some(EffortLevel::High)
    );
}

/// A session that is not live is checked like a new one when it is
/// recovered. Recovery overrides that move it to a model that does not
/// accept its durable level give a build that is refused typed, as the
/// session error a surface answers; the stored session is untouched and no
/// provider request is made. A recovery with no override builds.
#[tokio::test]
async fn a_recovered_session_is_checked_like_a_new_one() {
    use meerkat_core::session_recovery::{
        SurfaceSessionRecoveryContext, SurfaceSessionRecoveryOverrides, build_recovered_session,
    };
    use meerkat_session::ephemeral::SessionAgentBuilder as _;

    let harness = Harness::new();
    let durable = harness.durable_wide_session_at_max().await;
    let before = durable_identity(&durable);
    let builder = crate::FactoryAgentBuilder::new(
        scripted_factory(harness.store_path.clone(), &harness.script),
        test_config(),
    );

    let moved = build_recovered_session(
        durable.clone(),
        &SurfaceSessionRecoveryOverrides {
            model: Some(NARROW_MODEL.to_string()),
            ..Default::default()
        },
        SurfaceSessionRecoveryContext::default(),
    )
    .expect("the recovery overrides resolve");
    assert_eq!(
        moved
            .build
            .provider_params
            .as_ref()
            .and_then(|params| params.reasoning_effort),
        Some(EffortLevel::Max),
        "recovery keeps the durable level"
    );
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let error = match builder
        .build_agent(&moved.into_deferred_create_request(), tx)
        .await
    {
        Ok(_) => panic!("the recovery build must be refused"),
        Err(error) => error,
    };
    assert_eq!(
        meerkat_contracts::error::session_transport_error_code(&error),
        meerkat_contracts::ErrorCode::InvalidParams
    );
    assert_eq!(
        error.structured_data(),
        Some(json!({
            "kind": "reasoning_effort_refused",
            "retryable": false,
            "reasoning_effort_refusal": narrow_model_refusal_document(),
        }))
    );
    assert_eq!(
        durable_identity(&durable),
        before,
        "the session the recovery read is untouched"
    );
    assert!(harness.script.served().is_empty());

    let unchanged = build_recovered_session(
        durable,
        &SurfaceSessionRecoveryOverrides::default(),
        SurfaceSessionRecoveryContext::default(),
    )
    .expect("a recovery with no override resolves");
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    if let Err(error) = builder
        .build_agent(&unchanged.into_deferred_create_request(), tx)
        .await
    {
        panic!("a recovery on the session's own model builds: {error}");
    }
}

/// A live reconfiguration (hot swap) is checked like a build, against the
/// route of the client built for the target identity.
#[cfg(feature = "session-store")]
mod live {
    use super::*;
    use meerkat_core::SessionId;
    use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
    use meerkat_core::service::{
        CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
    };
    use meerkat_runtime::completion::CompletionOutcome;
    use meerkat_runtime::{
        Input, MeerkatMachine, PromptInput, RuntimeDriverError, SessionLlmReconfigureRequest,
        SessionServiceRuntimeExt,
    };

    use crate::{FactoryAgentBuilder, PersistenceBundle, PersistentSessionService, SessionStore};

    struct LiveHarness {
        _temp: tempfile::TempDir,
        script: Arc<Script>,
        service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
        adapter: Arc<MeerkatMachine>,
        session_id: SessionId,
    }

    fn create_request() -> CreateSessionRequest {
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: WIDE_MODEL.to_string(),
            prompt: meerkat_core::ContentInput::Text(String::new()),
            system_prompt: crate::SystemPromptOverride::Set("reasoning effort swap".to_string()),
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(SessionBuildOptions {
                provider: Some(Provider::Anthropic),
                realm_id: Some(RealmId::parse(REALM).expect("valid realm")),
                auth_binding: Some(binding(ANTHROPIC_BINDING)),
                override_image_generation: ToolCategoryOverride::Disable,
                ..Default::default()
            }),
            labels: None,
        }
    }

    impl LiveHarness {
        /// A runtime-backed session of [`WIDE_MODEL`] with no level, after
        /// one completed turn (so the session has a live actor).
        async fn new() -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let script = Arc::new(Script::default());
            let builder = FactoryAgentBuilder::new(
                scripted_factory(temp.path().join("factory-sessions"), &script),
                test_config(),
            );
            let session_store: Arc<dyn SessionStore> =
                Arc::new(meerkat_store::JsonlStore::new(temp.path().join("jsonl")));
            let persistence = PersistenceBundle::new(
                session_store,
                Arc::new(
                    meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(
                        temp.path().join("runtime.sqlite3"),
                    )
                    .expect("open the runtime store"),
                ),
                Arc::new(meerkat_store::MemoryBlobStore::new()),
            )
            .expect("construct runtime authority");
            let (service, adapter) =
                crate::surface::build_runtime_backed_service_with_default_reconfigure_host(
                    builder,
                    4,
                    persistence,
                    temp.path().join("config_state.json"),
                )
                .expect("compose the runtime-backed service");
            let session = Session::new();
            let session_id = session.id().clone();
            Box::pin(crate::surface::materialize_session(
                &service,
                &adapter,
                session,
                create_request(),
                {
                    let service = Arc::clone(&service);
                    let adapter = Arc::clone(&adapter);
                    move |session_id| {
                        crate::surface::default_persistent_executor(service, adapter, session_id)
                    }
                },
            ))
            .await
            .expect("materialize the runtime-backed session");
            let harness = Self {
                _temp: temp,
                script,
                service,
                adapter,
                session_id,
            };
            harness.run_turn("first turn, no level").await;
            harness
        }

        async fn run_turn(&self, prompt: &str) {
            let (_, completion) = self
                .adapter
                .accept_input_with_completion(
                    &self.session_id,
                    Input::Prompt(PromptInput::new(prompt, None)),
                )
                .await
                .expect("the runtime accepts the prompt");
            let outcome = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                completion.expect("a completion handle").wait(),
            )
            .await
            .expect("the turn settles in time")
            .expect("the completion resolves");
            assert!(
                matches!(outcome, CompletionOutcome::Completed(ref run) if run.text == ANSWER),
                "{prompt}: {outcome:?}"
            );
        }

        async fn durable_identity(&self) -> SessionLlmIdentity {
            self.service
                .load_authoritative_session(&self.session_id)
                .await
                .expect("load the session")
                .expect("the session is durable")
                .session_metadata()
                .expect("canonical metadata")
                .llm_identity()
        }

        async fn reconfigure(
            &self,
            model: Option<&str>,
            effort: Option<TurnMetadataOverride<EffortLevel>>,
        ) -> Result<meerkat_runtime::SessionLlmReconfigureReport, RuntimeDriverError> {
            self.adapter
                .reconfigure_session_llm_identity(
                    &self.session_id,
                    SessionLlmReconfigureRequest {
                        model: model.map(str::to_string),
                        provider: None,
                        self_hosted_server_id: None,
                        provider_params: None,
                        auth_binding: None,
                        reasoning_effort: effort,
                    },
                )
                .await
        }
    }

    /// A hot swap to a model whose route does not accept the session's level
    /// is refused with the typed carrier, and leaves the session exactly as
    /// it was: the durable identity (model, provider, params with the level,
    /// binding) is unchanged, the live session still runs the identity it
    /// had, and no provider request was made. The caller can then swap with
    /// a level the new model accepts in the same request.
    ///
    /// The refused request names a model and nothing else: the request a
    /// change of the global default model sends to each live session through
    /// this same entry, so such a session keeps its model and its level.
    #[tokio::test]
    async fn hot_swap_to_a_model_without_the_level_is_refused() {
        let harness = LiveHarness::new().await;

        // The session takes `max` on the model whose row accepts it.
        let report = harness
            .reconfigure(None, Some(TurnMetadataOverride::Set(EffortLevel::Max)))
            .await
            .unwrap_or_else(|error| panic!("a level the route accepts is applied: {error}"));
        let before = harness.durable_identity().await;
        assert_eq!(report.new_identity, before);
        assert_eq!(before.model, WIDE_MODEL);
        assert_eq!(
            before
                .provider_params
                .as_ref()
                .and_then(|params| params.reasoning_effort),
            Some(EffortLevel::Max),
            "the stored params keep the level as the neutral field"
        );
        assert_eq!(before.auth_binding, Some(binding(ANTHROPIC_BINDING)));
        harness.script.clear_served();

        // A model-only swap keeps the level, and the new route refuses it.
        let error = match harness.reconfigure(Some(NARROW_MODEL), None).await {
            Ok(report) => panic!("the swap must be refused, applied: {report:?}"),
            Err(error) => error,
        };
        let RuntimeDriverError::ReasoningEffortRefused(refusal) = &error else {
            panic!("expected the typed reasoning effort refusal, got: {error:?}");
        };
        assert_eq!(refusal_json(refusal), narrow_model_refusal_document());
        assert_eq!(
            error
                .reasoning_effort_refused_session_error()
                .and_then(|session_error| session_error.structured_data()),
            Some(json!({
                "kind": "reasoning_effort_refused",
                "retryable": false,
                "reasoning_effort_refusal": narrow_model_refusal_document(),
            })),
            "the refused swap has the data of a refused build"
        );
        assert_eq!(
            harness.durable_identity().await,
            before,
            "a refused swap leaves the stored identity and params as they were"
        );
        assert_eq!(
            harness
                .service
                .live_session_llm_identity(&harness.session_id)
                .await
                .expect("the live session's identity"),
            before,
            "a refused swap stages nothing: the live session keeps its model and its level"
        );
        assert!(
            harness.script.served().is_empty(),
            "a refused swap makes no provider request: {:?}",
            harness.script.served()
        );

        // The caller sets a level the new model accepts in the same swap.
        // Its report shows the identity the live session still had.
        let report = harness
            .reconfigure(
                Some(NARROW_MODEL),
                Some(TurnMetadataOverride::Set(EffortLevel::High)),
            )
            .await
            .unwrap_or_else(|error| panic!("a swap with an accepted level is applied: {error}"));
        assert_eq!(
            report.previous_identity, before,
            "the refused swap left the live identity in place"
        );
        assert_eq!(report.new_identity.model, NARROW_MODEL);
        assert_eq!(
            report
                .new_identity
                .provider_params
                .as_ref()
                .and_then(|params| params.reasoning_effort),
            Some(EffortLevel::High)
        );
        assert_eq!(harness.durable_identity().await, report.new_identity);
    }

    /// A hot swap that clears the level with the model change is not judged:
    /// the target carries no level.
    #[tokio::test]
    async fn hot_swap_that_clears_the_level_is_applied() {
        let harness = LiveHarness::new().await;
        harness
            .reconfigure(None, Some(TurnMetadataOverride::Set(EffortLevel::Max)))
            .await
            .unwrap_or_else(|error| panic!("a level the route accepts is applied: {error}"));
        let report = harness
            .reconfigure(Some(NARROW_MODEL), Some(TurnMetadataOverride::Clear))
            .await
            .unwrap_or_else(|error| panic!("a swap that clears the level is applied: {error}"));
        assert_eq!(report.new_identity.model, NARROW_MODEL);
        assert_eq!(
            report
                .new_identity
                .provider_params
                .as_ref()
                .and_then(|params| params.reasoning_effort),
            None
        );
    }
}
