//! A model only a GitHub Copilot account defines builds from the account's
//! model list.
//!
//! The model is in no catalog row and no `[models.<id>]` entry. The build
//! names a route of the account; the account's model list states the model,
//! the protocol it is served on and its limits. The build registers the
//! model for itself from that statement and runs on the account's route of
//! that protocol's provider family, never on another credential. A model
//! the account does not offer is refused typed.
//!
//! The provider layer is a credential-free fake `ProviderRuntime` per
//! provider in an otherwise empty `ProviderRuntimeRegistry` (as in
//! `resume_provider_override_binding_tests`): binding selection, catalog
//! validation and the factory's build are real, and the account's model list
//! is the fake runtime's answer to `ProviderRuntime::account_model`.

use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::config::CustomModelConfig;
use meerkat_core::{
    AuthBindingRef, BindingId, Config, Provider, RealmId, SessionLlmIdentity, StopReason,
    ToolCategoryOverride,
};
use meerkat_llm_core::provider_runtime::{
    AccountModelLookup, AccountModelRefusal, ProviderAuthError, ProviderClientError,
    ProviderRuntime, ProviderRuntimeRegistry, ResolvedConnection, ResolvedTextTarget,
    ResolverEnvironment, StaticLease, ValidatedBinding,
};

use crate::{AgentBuildConfig, AgentFactory, BuildAgentError, DynAgent};

/// A model the catalog does not know, which the account offers.
const ACCOUNT_MODEL: &str = "account-model-1";
const OPENAI_ROUTE: &str = "copilot_openai";
const ANTHROPIC_ROUTE: &str = "copilot_anthropic";
const PROMPT: &str = "hello from the account";

/// The account's two routes, as explicit device sign-in tables on the shared
/// `github_copilot` account.
const ACCOUNT_ROUTES: &str = r#"
[realm.global.backend.copilot_openai]
provider = "openai"
backend_kind = "copilot"

[realm.global.auth.copilot_openai]
provider = "openai"
auth_method = "github_copilot_oauth"
source = { kind = "managed_store" }

[realm.global.binding.copilot_openai]
backend_profile = "copilot_openai"
auth_profile = "copilot_openai"
credential_account = "github_copilot"

[realm.global.backend.copilot_anthropic]
provider = "anthropic"
backend_kind = "copilot"

[realm.global.auth.copilot_anthropic]
provider = "anthropic"
auth_method = "github_copilot_oauth"
source = { kind = "managed_store" }

[realm.global.binding.copilot_anthropic]
backend_profile = "copilot_anthropic"
auth_profile = "copilot_anthropic"
credential_account = "github_copilot"
"#;

/// The account configured once, in `[realm.global.copilot]`.
const ACCOUNT_SECTION: &str = r#"
[realm.global.copilot]
source = { kind = "env", env = "COPILOT_GITHUB_TOKEN" }
"#;

fn route(binding: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::global(),
        binding: BindingId::parse(binding).expect("valid binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

fn github_copilot_credential() -> meerkat_core::AuthCredentialIdentity {
    meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
        realm: RealmId::global(),
        account: meerkat_core::CredentialAccountId::parse("github_copilot").expect("valid account"),
    })
}

/// The account's statement of [`ACCOUNT_MODEL`]: served on the protocol of
/// `provider`, with its own limits.
fn account_definition(provider: Provider) -> CustomModelConfig {
    CustomModelConfig {
        provider,
        display_name: Some("Account Model".to_string()),
        context_window: Some(256_000),
        max_input_tokens: Some(192_000),
        max_output_tokens: Some(64_000),
        vision: Some(false),
        web_search: None,
        call_timeout_secs: None,
    }
}

fn answer_for(provider: Provider) -> String {
    format!("answered by {}", provider.as_str())
}

/// What the fake provider layer observed, in order.
#[derive(Default)]
struct ProviderLog {
    /// Every binding a provider runtime resolved.
    resolved: StdMutex<Vec<(Provider, AuthBindingRef)>>,
    /// Every account model question: the runtime asked and the model.
    asked: StdMutex<Vec<(Provider, String)>>,
    /// Every provider call: the provider that served it and its model.
    served: StdMutex<Vec<(Provider, String)>>,
}

impl ProviderLog {
    fn resolved(&self) -> Vec<(Provider, AuthBindingRef)> {
        self.resolved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn asked(&self) -> Vec<(Provider, String)> {
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn served(&self) -> Vec<(Provider, String)> {
        self.served
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn clear(&self) {
        self.resolved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.served
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

/// Answers every request with one text turn naming its provider.
struct ScriptedClient {
    provider: Provider,
    log: Arc<ProviderLog>,
}

#[async_trait]
impl LlmClient for ScriptedClient {
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
        self.log
            .served
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((self.provider, request.model.clone()));
        Box::pin(futures::stream::iter(vec![
            Ok(LlmEvent::TextDelta {
                delta: answer_for(self.provider),
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

/// Credential-free provider runtime of one provider family whose accounts
/// state their models: `offered` is the account's statement of
/// [`ACCOUNT_MODEL`] (`None`: the account does not offer it).
struct AccountRuntime {
    provider: Provider,
    log: Arc<ProviderLog>,
    offered: Option<CustomModelConfig>,
}

#[async_trait]
impl ProviderRuntime for AccountRuntime {
    fn provider_id(&self) -> Provider {
        self.provider
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        _env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        self.log
            .resolved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((self.provider, binding.auth_binding_ref().clone()));
        Ok(ResolvedConnection {
            provider: self.provider,
            backend: binding.backend(),
            backend_profile: Arc::clone(binding.backend_profile()),
            credential_identity: binding.credential_identity().clone(),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "unused-test-key".to_string(),
                meerkat_core::AuthMetadata::default(),
                None,
                format!("account-model-test:{:?}", self.provider),
            )),
        })
    }

    fn account_model(&self, connection: &ResolvedConnection, model: &str) -> AccountModelLookup {
        self.log
            .asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((self.provider, model.to_string()));
        match self.offered.as_ref() {
            Some(definition) if model == ACCOUNT_MODEL => {
                AccountModelLookup::Offered(definition.clone())
            }
            _ => AccountModelLookup::Refused(AccountModelRefusal::NotOffered {
                credential: connection.credential_identity.clone(),
                model: model.to_string(),
                available: Vec::new(),
            }),
        }
    }

    fn build_client(
        &self,
        _connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Ok(Arc::new(ScriptedClient {
            provider: self.provider,
            log: Arc::clone(&self.log),
        }))
    }

    fn build_text_client(
        &self,
        _target: ResolvedTextTarget,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Ok(Arc::new(ScriptedClient {
            provider: self.provider,
            log: Arc::clone(&self.log),
        }))
    }
}

struct Harness {
    _temp: tempfile::TempDir,
    log: Arc<ProviderLog>,
    factory: AgentFactory,
    config: Config,
}

impl Harness {
    /// A factory over the account's routes, whose account offers
    /// [`ACCOUNT_MODEL`] as `offered` states it.
    fn new(routes: &str, offered: Option<CustomModelConfig>) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let log = Arc::new(ProviderLog::default());
        let mut factory = AgentFactory::new(temp.path().join("sessions"))
            .without_provider_auth_persistence()
            .builtins(false);
        let mut registry = ProviderRuntimeRegistry::empty();
        for provider in [Provider::OpenAI, Provider::Anthropic, Provider::Gemini] {
            registry = registry.with_runtime(Arc::new(AccountRuntime {
                provider,
                log: Arc::clone(&log),
                offered: offered.clone(),
            }));
        }
        factory.provider_registry = Arc::new(registry);
        Self {
            _temp: temp,
            log,
            factory,
            config: toml::from_str(routes).expect("the account's routes"),
        }
    }

    /// A build of `model` on the account route `binding`, with the provider
    /// a create-session resolution gives it (the named route's).
    fn build_config(&self, model: &str, provider: Provider, binding: &str) -> AgentBuildConfig {
        let mut build = AgentBuildConfig::new(model);
        build.provider = Some(provider);
        build.auth_binding = Some(route(binding));
        build.override_image_generation = ToolCategoryOverride::Disable;
        build
    }

    async fn build(
        &self,
        model: &str,
        provider: Provider,
        binding: &str,
    ) -> Result<DynAgent, BuildAgentError> {
        self.factory
            .build_agent(self.build_config(model, provider, binding), &self.config)
            .await
    }
}

fn durable_identity(agent: &DynAgent) -> SessionLlmIdentity {
    agent
        .session()
        .session_metadata()
        .expect("the session persists its metadata")
        .llm_identity()
}

/// A model only the account defines builds from the account's statement of
/// it, on the account's route of the protocol the account serves it on: the
/// build named the account's Anthropic route, the account serves the model
/// on the OpenAI protocol, and the session runs on the account's OpenAI
/// route. No route outside the account resolves.
#[tokio::test]
async fn a_model_only_the_account_defines_builds_on_the_route_its_protocol_needs() {
    let harness = Harness::new(ACCOUNT_ROUTES, Some(account_definition(Provider::OpenAI)));
    let mut agent = harness
        .build(ACCOUNT_MODEL, Provider::Anthropic, ANTHROPIC_ROUTE)
        .await
        .unwrap_or_else(|error| panic!("a model the account defines builds: {error}"));
    let reply = agent
        .run(PROMPT.to_string().into())
        .await
        .expect("the turn runs on the account");
    assert_eq!(reply.text, answer_for(Provider::OpenAI));
    assert_eq!(
        harness.log.served(),
        vec![(Provider::OpenAI, ACCOUNT_MODEL.to_string())],
        "the OpenAI protocol serves the account model"
    );
    let identity = durable_identity(&agent);
    assert_eq!(
        (
            identity.provider,
            identity.model.as_str(),
            &identity.auth_binding
        ),
        (Provider::OpenAI, ACCOUNT_MODEL, &Some(route(OPENAI_ROUTE))),
        "the session runs on the account's route of the model's protocol"
    );
    let resolved = harness.log.resolved();
    assert!(
        !resolved.is_empty()
            && resolved
                .iter()
                .all(|(_, binding)| *binding == route(ANTHROPIC_ROUTE)
                    || *binding == route(OPENAI_ROUTE)),
        "only the account's routes resolve: {resolved:?}"
    );
}

/// The same with the account configured once (`[realm.global.copilot]`):
/// the account's routes come from the section, and the build moves from the
/// named route to the one of the model's protocol.
#[tokio::test]
async fn a_model_only_the_account_defines_builds_on_a_configured_once_account() {
    let harness = Harness::new(
        ACCOUNT_SECTION,
        Some(account_definition(Provider::Anthropic)),
    );
    let mut agent = harness
        .build(ACCOUNT_MODEL, Provider::OpenAI, OPENAI_ROUTE)
        .await
        .unwrap_or_else(|error| panic!("a model the account defines builds: {error}"));
    let reply = agent
        .run(PROMPT.to_string().into())
        .await
        .expect("the turn runs on the account");
    assert_eq!(reply.text, answer_for(Provider::Anthropic));
    let identity = durable_identity(&agent);
    assert_eq!(
        (identity.provider, &identity.auth_binding),
        (Provider::Anthropic, &Some(route(ANTHROPIC_ROUTE)))
    );
}

/// A model the account does not offer is refused with the account's typed
/// refusal, before any client is built, and no other route or credential
/// is tried in its place.
#[tokio::test]
async fn a_model_the_account_does_not_offer_is_refused_typed_without_another_credential() {
    let harness = Harness::new(ACCOUNT_ROUTES, None);
    match harness
        .build(ACCOUNT_MODEL, Provider::OpenAI, OPENAI_ROUTE)
        .await
    {
        Err(BuildAgentError::LlmClient(meerkat_client::FactoryError::AccountModel(
            AccountModelRefusal::NotOffered {
                credential, model, ..
            },
        ))) => {
            assert_eq!(model, ACCOUNT_MODEL);
            assert_eq!(credential, github_copilot_credential());
        }
        Err(other) => panic!("expected the account's typed refusal, got {other}"),
        Ok(_) => panic!("a model the account does not offer must not build"),
    }
    assert_eq!(
        harness.log.resolved(),
        vec![(Provider::OpenAI, route(OPENAI_ROUTE))],
        "only the named account route resolves; no other credential is tried"
    );
    assert!(harness.log.served().is_empty(), "nothing is sent");
}

/// A model the registry defines is never asked about: the catalog keeps
/// owning it, as before.
#[tokio::test]
async fn a_registered_model_never_asks_the_account() {
    let harness = Harness::new(ACCOUNT_ROUTES, Some(account_definition(Provider::OpenAI)));
    let catalog_model =
        meerkat_models::default_model(Provider::OpenAI).expect("an OpenAI catalog model");
    let mut agent = harness
        .build(catalog_model, Provider::OpenAI, OPENAI_ROUTE)
        .await
        .unwrap_or_else(|error| panic!("a catalog model builds on the account route: {error}"));
    agent
        .run(PROMPT.to_string().into())
        .await
        .expect("the turn runs");
    assert!(
        harness.log.asked().is_empty(),
        "a registered model never asks the account: {:?}",
        harness.log.asked()
    );
}

/// A resumed session on a model only the account defines reads the
/// account's model list again (nothing about the model is persisted) and
/// keeps its route.
#[tokio::test]
async fn a_resumed_session_on_an_account_model_reads_the_account_again() {
    let harness = Harness::new(ACCOUNT_ROUTES, Some(account_definition(Provider::OpenAI)));
    let mut agent = harness
        .build(ACCOUNT_MODEL, Provider::OpenAI, OPENAI_ROUTE)
        .await
        .unwrap_or_else(|error| panic!("a model the account defines builds: {error}"));
    agent
        .run(PROMPT.to_string().into())
        .await
        .expect("the first turn runs");
    let durable = agent.session().clone();
    drop(agent);
    harness.log.clear();

    let mut resume = AgentBuildConfig::new(ACCOUNT_MODEL);
    resume.override_image_generation = ToolCategoryOverride::Disable;
    resume.resume_session = Some(durable);
    let mut resumed = harness
        .factory
        .build_agent(resume, &harness.config)
        .await
        .unwrap_or_else(|error| panic!("the account model session resumes: {error}"));
    let reply = resumed
        .run(PROMPT.to_string().into())
        .await
        .expect("the resumed turn runs");
    assert_eq!(reply.text, answer_for(Provider::OpenAI));
    assert_eq!(
        harness.log.asked(),
        vec![(Provider::OpenAI, ACCOUNT_MODEL.to_string())],
        "the resume asks the account again"
    );
    let identity = durable_identity(&resumed);
    assert_eq!(
        (identity.provider, &identity.auth_binding),
        (Provider::OpenAI, &Some(route(OPENAI_ROUTE)))
    );
}

/// A create-session request naming any route of a configured-once account
/// is served on the account's route of the model's provider: the route
/// names the account, not a protocol. A request that names its provider
/// keeps the strict binding check.
#[test]
fn a_route_of_a_copilot_account_serves_a_catalog_model_on_its_provider_route() {
    let config: Config = toml::from_str(ACCOUNT_SECTION).expect("the account section");
    let anthropic_model =
        meerkat_models::default_model(Provider::Anthropic).expect("an Anthropic catalog model");
    let resolution = crate::resolve_create_session_model(
        &config,
        crate::CreateSessionModelResolutionRequest {
            model: Some(anthropic_model.to_string()),
            provider: None,
            auth_binding: Some(route(OPENAI_ROUTE)),
        },
    )
    .unwrap_or_else(|error| panic!("the account serves the catalog model: {error}"));
    assert_eq!(resolution.provider, Provider::Anthropic);
    assert_eq!(resolution.auth_binding, Some(route(ANTHROPIC_ROUTE)));

    let strict = crate::resolve_create_session_model(
        &config,
        crate::CreateSessionModelResolutionRequest {
            model: Some(anthropic_model.to_string()),
            provider: Some(Provider::OpenAI),
            auth_binding: Some(route(OPENAI_ROUTE)),
        },
    );
    assert!(
        strict.is_err(),
        "a named provider keeps the strict check: {strict:?}"
    );
}

/// The routes of a configured-once account keep their account as their
/// credential identity outside a build too: a fallback target moves to the
/// account's route of its provider, its request policy names the same
/// account, and the Copilot routes keep provider-native tools off.
#[test]
fn configured_once_copilot_routes_keep_their_account_through_a_fallback() {
    let mut config: Config = toml::from_str(ACCOUNT_SECTION).expect("the account section");
    config.model_fallback.enabled = Some(true);
    config.model_fallback.policy.cross_provider = true;
    config.model_fallback.policy.trigger_after_attempts = 1;
    config.model_fallback.policy.require_tool_parity = false;
    config.model_fallback.chain = vec![meerkat_core::config::ModelFallbackTarget {
        model: meerkat_models::default_model(Provider::Anthropic)
            .expect("Anthropic default")
            .to_string(),
        provider: Some(Provider::Anthropic),
        auth_binding: None,
    }];
    let factory = AgentFactory::minimal();
    let current = SessionLlmIdentity {
        model: meerkat_models::default_model(Provider::OpenAI)
            .expect("OpenAI default")
            .to_string(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(route(OPENAI_ROUTE)),
    };
    let current_credential = AgentFactory::credential_identity_for_llm_identity(&config, &current)
        .expect("current credential");
    assert_eq!(current_credential, Some(github_copilot_credential()));

    let registry = config
        .model_registry(meerkat_models::canonical())
        .expect("registry");
    let (candidates, skipped) = factory
        .model_fallback_identities(&config, &registry, &current)
        .expect("fallback identities");
    assert!(skipped.is_empty(), "{skipped:?}");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].auth_binding, Some(route(ANTHROPIC_ROUTE)));

    let fallback_policy = factory
        .request_policy_for_llm_identity(&config, &candidates[0], ToolCategoryOverride::Inherit)
        .expect("fallback policy");
    assert_eq!(fallback_policy.credential_identity, current_credential);
    assert_eq!(
        fallback_policy.provider_native_tools,
        meerkat_core::ProviderNativeToolPolicy::DisableAll
    );
}

/// A session created on a zero-config Copilot route resumes on that route.
/// Its durable identity names the route (provider, model and binding),
/// never a credential identity, so nothing durable differs from a session
/// created before account routes resolved through their account. The
/// rebuilt session's credential identity is the route's account: the
/// identity its connection and the account's AuthMachine lease have always
/// used (it was the binding's own before, a key no lease of the route
/// carried).
#[tokio::test]
async fn a_zero_config_session_resumes_on_its_route_with_its_account_identity() {
    let harness = Harness::new("", Some(account_definition(Provider::OpenAI)));
    let catalog_model =
        meerkat_models::default_model(Provider::OpenAI).expect("an OpenAI catalog model");
    let mut agent = harness
        .build(catalog_model, Provider::OpenAI, OPENAI_ROUTE)
        .await
        .unwrap_or_else(|error| panic!("a catalog model builds on a zero-config route: {error}"));
    agent
        .run(PROMPT.to_string().into())
        .await
        .expect("the first turn runs");
    let created = durable_identity(&agent);
    assert_eq!(
        (created.provider, &created.auth_binding),
        (Provider::OpenAI, &Some(route(OPENAI_ROUTE))),
        "the durable identity names the zero-config route"
    );
    let durable = agent.session().clone();
    drop(agent);
    harness.log.clear();

    let mut resume = AgentBuildConfig::new(catalog_model);
    resume.override_image_generation = ToolCategoryOverride::Disable;
    resume.resume_session = Some(durable);
    let mut resumed = harness
        .factory
        .build_agent(resume, &harness.config)
        .await
        .unwrap_or_else(|error| panic!("the zero-config session resumes: {error}"));
    resumed
        .run(PROMPT.to_string().into())
        .await
        .expect("the resumed turn runs");
    let identity = durable_identity(&resumed);
    assert_eq!(
        (identity.provider, &identity.model, &identity.auth_binding),
        (created.provider, &created.model, &created.auth_binding),
        "the resumed session keeps its route"
    );
    assert_eq!(
        harness.log.resolved(),
        vec![(Provider::OpenAI, route(OPENAI_ROUTE))],
        "the resume resolves the session's own route"
    );
    assert_eq!(
        AgentFactory::credential_identity_for_llm_identity(&harness.config, &identity)
            .expect("the credential identity"),
        Some(github_copilot_credential()),
        "the session's credential identity is the route's account"
    );
}

/// Create-session account affinity never leaves its credential account. An
/// account whose routes are of one provider (every account but a Copilot
/// one: its contract is that provider's own auth method) has no route for
/// another provider, so a model of another provider keeps the strict
/// mismatch.
#[test]
fn a_non_copilot_credential_account_keeps_the_strict_mismatch() {
    let config: Config = toml::from_str(
        r#"
[realm.global.backend.openai_key]
provider = "openai"
backend_kind = "openai_api"

[realm.global.auth.openai_key]
provider = "openai"
auth_method = "api_key"
source = { kind = "env", env = "OPENAI_API_KEY" }

[realm.global.binding.openai_key]
backend_profile = "openai_key"
auth_profile = "openai_key"
credential_account = "team_key"
"#,
    )
    .expect("an API key route on a credential account");
    let anthropic_model =
        meerkat_models::default_model(Provider::Anthropic).expect("an Anthropic catalog model");
    let error = crate::resolve_create_session_model(
        &config,
        crate::CreateSessionModelResolutionRequest {
            model: Some(anthropic_model.to_string()),
            provider: None,
            auth_binding: Some(route("openai_key")),
        },
    )
    .expect_err("a model of another provider than the account's routes is refused");
    assert!(
        matches!(
            &error,
            crate::CreateSessionModelResolutionError::ProviderModelMismatch(_)
        ),
        "{error}"
    );
}
