//! A durable session resumed under a provider override moves to that provider's
//! credential binding.
//!
//! The scenario is the one a mob profile edit produces: a session created on
//! OpenAI persists the realm binding it resolved (`global:openai`), the profile
//! then moves to Anthropic with `resume_overrides = ["model", "provider"]`, and
//! the next resume carries the profile's model and provider through
//! `ResumeOverrideMask { model, provider }` with no explicit binding of its own.
//! The resumed session must keep its history and run its next turn on Anthropic
//! through the binding the realm configures for Anthropic, exactly as a fresh
//! session of the same profile would.
//!
//! The provider layer is a credential-free fake `ProviderRuntime` per provider
//! in an otherwise empty `ProviderRuntimeRegistry`, so binding selection,
//! catalog validation and client construction are the factory's real path and
//! only the network is scripted. That is why this lives under `factory` (see
//! the note on `brain_swap_runtime_loop_ab_tests`): the registry slot is
//! private.

use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::{
    AgentToolDispatcher, AssistantBlock, AuthBindingRef, BackendProfileConfig, BindingId, Config,
    CredentialSourceSpec, Message, Provider, ProviderBindingConfig, RealmConfigSection, RealmId,
    Session, SessionLlmIdentity, StopReason, ToolCallView, ToolCategoryOverride, ToolDef,
    ToolDispatchOutcome, ToolError, ToolResult,
};
use meerkat_llm_core::provider_runtime::{
    ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
    ResolvedConnection, ResolvedTextTarget, ResolverEnvironment, StaticLease, ValidatedBinding,
};

use crate::{AgentBuildConfig, AgentFactory, BuildAgentError, DynAgent};

const GLOBAL: &str = "global";
const TENANT: &str = "tenant";
const OPENAI_BINDING: &str = "openai";
const ANTHROPIC_BINDING: &str = "anthropic";
const OPENAI_MODEL: &str = "gpt-5.4";
const OPENAI_MODEL_B: &str = "gpt-5.5";
const ANTHROPIC_MODEL: &str = "claude-sonnet-4-6";
const FIRST_PROMPT: &str = "first turn, on openai";
const SECOND_PROMPT: &str = "first turn after the provider switch";
const THIRD_PROMPT: &str = "first turn after a cold restart";
const ECHO_TOOL: &str = "echo";
const ECHO_CALL: &str = "call_echo_1";
const ACCOUNT: &str = "acct_a";

fn binding_ref(realm: &str, binding: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(realm).expect("valid realm"),
        binding: BindingId::parse(binding).expect("valid binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

fn realm_binding(binding: &str) -> AuthBindingRef {
    binding_ref(GLOBAL, binding)
}

/// One credential route in a realm section.
#[derive(Clone, Copy)]
enum Route {
    /// An API key binding for the provider.
    ApiKey(Provider),
    /// A binding backed by credential account [`ACCOUNT`] (GitHub Copilot
    /// OAuth, the provider-spanning account in the catalog).
    Account(Provider),
}

fn add_route(
    section: &mut RealmConfigSection,
    binding: &str,
    route: Route,
    provider_default: bool,
) {
    let (provider, backend_kind, auth_method, source, credential_account) = match route {
        Route::ApiKey(provider) => (
            provider,
            match provider {
                Provider::Anthropic => "anthropic_api",
                _ => "openai_api",
            },
            "api_key",
            CredentialSourceSpec::InlineSecret {
                secret: format!("unused-test-key-{binding}"),
            },
            None,
        ),
        Route::Account(provider) => (
            provider,
            "copilot",
            "github_copilot_oauth",
            CredentialSourceSpec::ManagedStore,
            Some(meerkat_core::CredentialAccountId::parse(ACCOUNT).expect("valid account")),
        ),
    };
    section.backend.insert(
        binding.to_string(),
        BackendProfileConfig {
            provider: provider.as_str().to_string(),
            backend_kind: backend_kind.to_string(),
            base_url: None,
            options: serde_json::Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        binding.to_string(),
        meerkat_core::AuthProfileConfig {
            provider: provider.as_str().to_string(),
            auth_method: auth_method.to_string(),
            source,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        binding.to_string(),
        ProviderBindingConfig {
            backend_profile: binding.to_string(),
            auth_profile: binding.to_string(),
            credential_account,
            default_model: None,
            policy: Default::default(),
            provider_default,
        },
    );
}

/// `[realm.global]` with the given routes; the first route is the realm
/// default (the session's OpenAI route in every scenario).
fn global_config(routes: &[(&str, Route, bool)]) -> Config {
    let mut section = RealmConfigSection {
        default_binding: routes.first().map(|(binding, _, _)| (*binding).to_string()),
        ..Default::default()
    };
    for (binding, route, provider_default) in routes {
        add_route(&mut section, binding, *route, *provider_default);
    }
    let mut config = Config::default();
    config.realm.insert(GLOBAL.to_string(), section);
    config
}

/// A host shape: one API-key binding per provider, OpenAI the realm default.
fn global_realm_config() -> Config {
    global_config(&[
        (OPENAI_BINDING, Route::ApiKey(Provider::OpenAI), false),
        (ANTHROPIC_BINDING, Route::ApiKey(Provider::Anthropic), false),
    ])
}

/// What the fake provider layer observed, in order.
#[derive(Default)]
struct ProviderLog {
    /// Every binding a provider runtime resolved.
    resolved: StdMutex<Vec<(Provider, AuthBindingRef)>>,
    /// Every provider call: which provider served it, for which model, and
    /// the user turns its request replayed.
    served: StdMutex<Vec<(Provider, String, Vec<String>)>>,
    /// The messages each Anthropic call would send after the Anthropic
    /// client's own replay projection.
    anthropic_projected: StdMutex<Vec<Vec<Message>>>,
}

impl ProviderLog {
    fn resolved(&self) -> Vec<(Provider, AuthBindingRef)> {
        self.resolved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn served(&self) -> Vec<(Provider, String, Vec<String>)> {
        self.served
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn anthropic_projected(&self) -> Vec<Vec<Message>> {
        self.anthropic_projected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn clear(&self) {
        self.resolved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.served
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.anthropic_projected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

fn user_turns(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .collect()
}

fn answer_for(provider: Provider) -> String {
    format!("answered by {}", provider.as_str())
}

/// Scripted client. On OpenAI with `tool_round`, the first call of a turn
/// emits an encrypted OpenAI reasoning item and an echo tool call, so the
/// transcript carries provider-native reasoning and a tool pair.
struct ScriptedClient {
    provider: Provider,
    log: Arc<ProviderLog>,
    tool_round: bool,
}

impl ScriptedClient {
    fn events(&self, request: &LlmRequest) -> Vec<Result<LlmEvent, LlmError>> {
        let answered_tool = matches!(request.messages.last(), Some(Message::ToolResults { .. }));
        let mut events = Vec::new();
        let stop_reason = if self.tool_round && self.provider == Provider::OpenAI && !answered_tool
        {
            events.push(Ok(LlmEvent::ReasoningComplete {
                text: "openai private reasoning".to_string(),
                meta: Some(Box::new(meerkat_core::ProviderMeta::OpenAi {
                    id: "rs_openai_1".to_string(),
                    encrypted_content: Some("encrypted-openai-reasoning".to_string()),
                    phase: None,
                    response_id: None,
                })),
            }));
            events.push(Ok(LlmEvent::ToolCallComplete {
                id: ECHO_CALL.to_string(),
                name: ECHO_TOOL.to_string(),
                args: serde_json::json!({}),
                meta: None,
            }));
            StopReason::ToolUse
        } else {
            events.push(Ok(LlmEvent::TextDelta {
                delta: answer_for(self.provider),
                meta: None,
            }));
            StopReason::EndTurn
        };
        events.push(Ok(LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                self.provider,
                &request.model,
                meerkat_core::Usage::default(),
            ),
        }));
        events.push(Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::success(stop_reason),
        }));
        events
    }
}

#[async_trait]
impl LlmClient for ScriptedClient {
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
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
            .push((
                self.provider,
                request.model.clone(),
                user_turns(&request.messages),
            ));
        #[cfg(feature = "anthropic")]
        if self.provider == Provider::Anthropic {
            // What the real Anthropic client would send for this history.
            let projected = meerkat_anthropic::AnthropicClient::new("unused-test-key".to_string())
                .and_then(|client| client.project_replay_messages(&request.messages));
            match projected {
                Ok(projected) => self
                    .log
                    .anthropic_projected
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(projected),
                Err(error) => {
                    return Box::pin(futures::stream::iter(vec![Err(error)]));
                }
            }
        }
        Box::pin(futures::stream::iter(self.events(request)))
    }

    fn provider(&self) -> Provider {
        self.provider
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Credential-free provider runtime: resolves the validated binding to a
/// static non-secret lease, records which binding it resolved, and builds the
/// scripted client. The synthetic env-default binding resolves only when the
/// scenario says the process environment holds the provider's key; otherwise
/// it fails exactly as a provider runtime does for an absent env secret.
struct RecordingProviderRuntime {
    provider: Provider,
    log: Arc<ProviderLog>,
    env_key_present: bool,
    tool_round: bool,
}

impl RecordingProviderRuntime {
    fn client(&self) -> Arc<dyn LlmClient> {
        Arc::new(ScriptedClient {
            provider: self.provider,
            log: Arc::clone(&self.log),
            tool_round: self.tool_round,
        })
    }
}

#[async_trait]
impl ProviderRuntime for RecordingProviderRuntime {
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
        if binding.auth_binding_ref().is_env_default() && !self.env_key_present {
            return Err(ProviderAuthError::Auth(
                meerkat_core::AuthError::MissingSecret,
            ));
        }
        Ok(ResolvedConnection {
            provider: self.provider,
            backend: binding.backend(),
            backend_profile: Arc::clone(binding.backend_profile()),
            credential_identity: binding.credential_identity().clone(),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "unused-test-key".to_string(),
                meerkat_core::AuthMetadata::default(),
                None,
                format!("resume-provider-override-test:{:?}", self.provider),
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

/// An echo tool, so a turn can leave a tool-use/tool-result pair behind.
struct EchoTool;

#[async_trait]
impl AgentToolDispatcher for EchoTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new(
            ECHO_TOOL,
            "Echo",
            serde_json::json!({"type": "object"}),
        ))]
        .into()
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Ok(ToolResult::new(call.id.into(), "echoed".into(), false).into())
    }
}

/// Scenario knobs.
#[derive(Clone, Copy, Default)]
struct Scenario {
    /// The process environment holds the provider keys (env-default route).
    env_key_present: bool,
    /// The OpenAI turn leaves reasoning and a tool pair in the transcript.
    tool_round: bool,
}

struct Harness {
    _temp: tempfile::TempDir,
    store_path: std::path::PathBuf,
    log: Arc<ProviderLog>,
    factory: AgentFactory,
    config: Config,
    tool_round: bool,
}

impl Harness {
    fn new(config: Config, scenario: Scenario) -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let store_path = temp.path().join("sessions");
        let log = Arc::new(ProviderLog::default());
        let mut factory = AgentFactory::new(store_path.clone())
            .without_provider_auth_persistence()
            .builtins(false);
        let mut registry = ProviderRuntimeRegistry::empty();
        for provider in [Provider::OpenAI, Provider::Anthropic] {
            registry = registry.with_runtime(Arc::new(RecordingProviderRuntime {
                provider,
                log: Arc::clone(&log),
                env_key_present: scenario.env_key_present,
                tool_round: scenario.tool_round,
            }));
        }
        factory.provider_registry = Arc::new(registry);
        Self {
            _temp: temp,
            store_path,
            log,
            factory,
            config,
            tool_round: scenario.tool_round,
        }
    }

    fn build_config(&self, model: &str, provider: Provider) -> AgentBuildConfig {
        let mut build = AgentBuildConfig::new(model);
        build.provider = Some(provider);
        build.override_image_generation = ToolCategoryOverride::Disable;
        if self.tool_round {
            build.external_tools = Some(Arc::new(EchoTool));
        }
        build
    }

    /// Create a session on OpenAI in `realm` (the global head when `None`)
    /// and run one turn. Returns the durable session.
    async fn create_openai_session(&self, realm: Option<&str>) -> Session {
        let mut create = self.build_config(OPENAI_MODEL, Provider::OpenAI);
        create.realm_id = realm.map(|realm| RealmId::parse(realm).expect("valid realm"));
        let mut agent = self
            .factory
            .build_agent(create, &self.config)
            .await
            .expect("the OpenAI session builds through the realm's OpenAI route");
        let first = agent
            .run(FIRST_PROMPT.to_string().into())
            .await
            .expect("the first turn runs on OpenAI");
        assert_eq!(first.text, answer_for(Provider::OpenAI));
        let durable = agent.session().clone();
        drop(agent);
        self.log.clear();
        durable
    }

    /// Resume `durable` as a profile moved to Anthropic with
    /// `resume_overrides = ["model", "provider"]`.
    async fn resume_switched(
        &self,
        durable: Session,
        realm: Option<&str>,
        explicit_binding: Option<AuthBindingRef>,
    ) -> Result<DynAgent, BuildAgentError> {
        let mut resume = self.build_config(ANTHROPIC_MODEL, Provider::Anthropic);
        resume.realm_id = realm.map(|realm| RealmId::parse(realm).expect("valid realm"));
        resume.resume_override_mask.model = true;
        resume.resume_override_mask.provider = true;
        resume.auth_binding = explicit_binding;
        resume.resume_session = Some(durable);
        self.factory.build_agent(resume, &self.config).await
    }

    /// Preflight the switch the way a member host does: the resume merge's
    /// identity, with the durable identity as the resume prior.
    async fn preflight_switched(
        &self,
        durable: &Session,
    ) -> Result<(), crate::LlmIdentityPreflightError> {
        let durable = durable
            .session_metadata()
            .expect("durable metadata")
            .llm_identity();
        let mask = meerkat_core::service::ResumeOverrideMask {
            model: true,
            provider: true,
            ..Default::default()
        };
        let identity = meerkat_core::resolve_resumed_session_llm_identity(
            SessionLlmIdentity {
                model: ANTHROPIC_MODEL.to_string(),
                provider: Provider::Anthropic,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            },
            &durable,
            &mask,
        );
        self.factory
            .preflight_resumed_llm_identity(
                &self.config,
                &identity,
                Some(&durable),
                &std::collections::BTreeMap::new(),
                None,
                None,
            )
            .await
    }

    /// Resume `durable` with no override: durable truth wins, as on a cold
    /// restart of an unchanged profile.
    async fn resume_unchanged(&self, durable: Session) -> Result<DynAgent, BuildAgentError> {
        let mut resume = self.build_config(OPENAI_MODEL, Provider::OpenAI);
        resume.provider = None;
        resume.resume_session = Some(durable);
        self.factory.build_agent(resume, &self.config).await
    }
}

fn expect_switched(result: Result<DynAgent, BuildAgentError>) -> DynAgent {
    result.unwrap_or_else(|error| {
        panic!(
            "a resume whose Provider override moves the session to Anthropic must resolve the \
             profile provider's credential route instead of the persisted OpenAI binding: {error}"
        )
    })
}

fn durable_identity(agent: &DynAgent) -> SessionLlmIdentity {
    agent
        .session()
        .session_metadata()
        .expect("the session persists its metadata")
        .llm_identity()
}

fn assert_never_resolved_openai(log: &ProviderLog) {
    let resolved = log.resolved();
    assert!(
        resolved
            .iter()
            .all(|(provider, binding)| *provider == Provider::Anthropic
                && *binding != realm_binding(OPENAI_BINDING)),
        "the switched build never resolves the persisted OpenAI binding: {resolved:?}"
    );
}

/// Create on OpenAI, then resume as a profile moved to Anthropic and run one
/// turn. `explicit_resume_binding` is the binding the resume names of its own
/// (a mob spawn spec's `auth_binding`); the profile itself has none.
async fn switch_a_durable_openai_session_to_anthropic(
    explicit_resume_binding: Option<AuthBindingRef>,
) {
    let harness = Harness::new(global_realm_config(), Scenario::default());
    let durable = harness.create_openai_session(None).await;
    let created = durable
        .session_metadata()
        .expect("the created session persists its metadata");
    assert_eq!(
        (created.provider, created.auth_binding.clone()),
        (Provider::OpenAI, Some(realm_binding(OPENAI_BINDING))),
        "fixture: the durable session names the OpenAI realm binding"
    );

    let mut resumed = expect_switched(
        harness
            .resume_switched(durable, None, explicit_resume_binding)
            .await,
    );
    let next = resumed
        .run(SECOND_PROMPT.to_string().into())
        .await
        .expect("the next turn runs after the provider switch");
    assert_eq!(next.text, answer_for(Provider::Anthropic));
    let resolved = harness.log.resolved();
    assert!(
        !resolved.is_empty()
            && resolved
                .iter()
                .all(|entry| *entry == (Provider::Anthropic, realm_binding(ANTHROPIC_BINDING))),
        "the resumed session resolves the realm's Anthropic binding, and never the persisted \
         OpenAI one: {resolved:?}"
    );
    assert_eq!(
        harness.log.served(),
        vec![(
            Provider::Anthropic,
            ANTHROPIC_MODEL.to_string(),
            vec![FIRST_PROMPT.to_string(), SECOND_PROMPT.to_string()],
        )],
        "the next turn is served by Anthropic and replays the history created on OpenAI"
    );
    let switched = durable_identity(&resumed);
    assert_eq!(
        (
            switched.provider,
            switched.model.as_str(),
            switched.auth_binding.clone()
        ),
        (
            Provider::Anthropic,
            ANTHROPIC_MODEL,
            Some(realm_binding(ANTHROPIC_BINDING))
        ),
        "the durable identity names the binding the switched session now runs on"
    );
}

/// R1. The reported gap: the resume names no binding, so the persisted OpenAI
/// binding is restored under the Anthropic provider and the build refuses
/// with `ConnectionTargetError::ProviderMismatch`.
#[tokio::test]
async fn provider_resume_override_rebinds_the_session_to_the_profile_providers_binding() {
    switch_a_durable_openai_session_to_anthropic(None).await;
}

/// Control: the same switch with the Anthropic binding named explicitly by
/// the resume. It passes today, which shows the scenario's assertions hold
/// once the binding is right: binding selection is the only gap.
#[tokio::test]
async fn provider_resume_override_with_an_explicit_binding_switches_today() {
    switch_a_durable_openai_session_to_anthropic(Some(realm_binding(ANTHROPIC_BINDING))).await;
}

/// R3 (guard). An explicit binding stays strict: naming the OpenAI binding
/// on a resume that switches to Anthropic is a `ProviderMismatch`, never a
/// silent fallback to another route.
#[tokio::test]
async fn explicit_wrong_provider_binding_on_a_switched_resume_stays_strict() {
    let harness = Harness::new(global_realm_config(), Scenario::default());
    let durable = harness.create_openai_session(None).await;
    let result = harness
        .resume_switched(durable, None, Some(realm_binding(OPENAI_BINDING)))
        .await;
    match result {
        Err(BuildAgentError::LlmClient(meerkat_client::FactoryError::ConnectionTarget(
            meerkat_core::ConnectionTargetError::ProviderMismatch {
                binding, expected, ..
            },
        ))) => {
            assert_eq!(binding, OPENAI_BINDING);
            assert_eq!(expected, Provider::Anthropic);
        }
        Err(other) => panic!("expected the strict ProviderMismatch, got {other}"),
        Ok(_) => panic!("an explicit wrong-provider binding must not build"),
    }
    assert!(
        harness.log.resolved().is_empty(),
        "nothing resolves for a refused explicit binding"
    );
}

/// R4. The switch resolves from the session's realm: a child realm's own
/// Anthropic binding beats the one it inherits from `global`, and the realm
/// that owns the durable (inherited) OpenAI binding is never the head.
#[tokio::test]
async fn provider_switch_resolves_from_the_session_realm_not_the_old_bindings_owner() {
    let mut config = global_realm_config();
    let mut tenant = RealmConfigSection {
        parent: Some(RealmId::global()),
        ..Default::default()
    };
    add_route(
        &mut tenant,
        "tenant_anthropic",
        Route::ApiKey(Provider::Anthropic),
        false,
    );
    config.realm.insert(TENANT.to_string(), tenant);
    let harness = Harness::new(config, Scenario::default());

    let durable = harness.create_openai_session(Some(TENANT)).await;
    assert_eq!(
        durable
            .session_metadata()
            .and_then(|metadata| metadata.auth_binding),
        Some(realm_binding(OPENAI_BINDING)),
        "fixture: the tenant session runs on the OpenAI binding it inherits from global"
    );

    let mut resumed = expect_switched(harness.resume_switched(durable, Some(TENANT), None).await);
    resumed
        .run(SECOND_PROMPT.to_string().into())
        .await
        .expect("the next turn runs after the provider switch");
    let tenant_anthropic = binding_ref(TENANT, "tenant_anthropic");
    let resolved = harness.log.resolved();
    assert!(
        !resolved.is_empty()
            && resolved
                .iter()
                .all(|entry| *entry == (Provider::Anthropic, tenant_anthropic.clone())),
        "the session realm's Anthropic binding wins over global's: {resolved:?}"
    );
    assert_eq!(
        durable_identity(&resumed).auth_binding,
        Some(tenant_anthropic)
    );
}

/// R5a. An account-backed durable binding keeps its account across the
/// switch: the route sharing the account wins over the realm's Anthropic
/// default.
#[cfg(feature = "copilot")]
#[tokio::test]
async fn account_backed_provider_switch_keeps_the_credential_account() {
    let config = global_config(&[
        ("openai_account", Route::Account(Provider::OpenAI), false),
        (
            "anthropic_account",
            Route::Account(Provider::Anthropic),
            false,
        ),
        ("anthropic_key", Route::ApiKey(Provider::Anthropic), true),
    ]);
    let harness = Harness::new(config, Scenario::default());
    let durable = harness.create_openai_session(None).await;
    assert_eq!(
        durable
            .session_metadata()
            .and_then(|metadata| metadata.auth_binding),
        Some(realm_binding("openai_account")),
        "fixture: the session runs on the account-backed OpenAI route"
    );

    // The member-host preflight reaches the build's route.
    harness
        .preflight_switched(&durable)
        .await
        .expect("the preflight resolves the account-sharing route");
    let preflighted = harness.log.resolved();
    assert!(
        !preflighted.is_empty()
            && preflighted
                .iter()
                .all(|entry| *entry == (Provider::Anthropic, realm_binding("anthropic_account"))),
        "the preflight keeps the credential account: {preflighted:?}"
    );
    harness.log.clear();

    let mut resumed = expect_switched(harness.resume_switched(durable, None, None).await);
    resumed
        .run(SECOND_PROMPT.to_string().into())
        .await
        .expect("the next turn runs after the provider switch");
    let resolved = harness.log.resolved();
    assert!(
        !resolved.is_empty()
            && resolved
                .iter()
                .all(|entry| *entry == (Provider::Anthropic, realm_binding("anthropic_account"))),
        "the switch keeps the credential account: {resolved:?}"
    );
    assert_eq!(
        durable_identity(&resumed).auth_binding,
        Some(realm_binding("anthropic_account"))
    );
}

/// R5b. An account-backed durable binding with no route sharing the account
/// for the new provider refuses typed. It never falls back to another
/// account's route or to the old binding.
#[cfg(feature = "copilot")]
#[tokio::test]
async fn account_backed_provider_switch_without_a_shared_route_refuses() {
    let config = global_config(&[
        ("openai_account", Route::Account(Provider::OpenAI), false),
        ("anthropic_key", Route::ApiKey(Provider::Anthropic), true),
    ]);
    let harness = Harness::new(config, Scenario::default());
    let durable = harness.create_openai_session(None).await;
    let expected = meerkat_core::ConnectionTargetError::NoCredentialAccountRoute {
        provider: Provider::Anthropic,
        account: meerkat_core::CredentialAccountRef {
            realm: RealmId::global(),
            account: meerkat_core::CredentialAccountId::parse(ACCOUNT).expect("valid account"),
        },
    };

    // The member-host preflight refuses with the build's typed error.
    match harness.preflight_switched(&durable).await {
        Err(crate::LlmIdentityPreflightError::BindingSelection { source }) => {
            assert_eq!(source, expected);
        }
        Err(other) => panic!("expected the typed BindingSelection refusal, got {other}"),
        Ok(()) => panic!("the preflight must refuse a switch with no account route"),
    }

    let error = match harness.resume_switched(durable, None, None).await {
        Ok(_) => panic!("a switch with no route sharing the account must refuse"),
        Err(error) => error,
    };
    let rendered = error.to_string();
    assert!(
        rendered.contains("has no route sharing credential account 'global:acct_a'"),
        "the refusal names the account the switch cannot keep: {rendered}"
    );
    match error {
        BuildAgentError::LlmClient(meerkat_client::FactoryError::ConnectionTarget(source)) => {
            assert_eq!(source, expected, "the build and the preflight refuse alike");
        }
        other => panic!("expected the typed NoCredentialAccountRoute refusal, got {other}"),
    }
    assert!(
        harness.log.resolved().is_empty(),
        "nothing resolves for a refused account switch"
    );
}

/// R6a. No realm binding serves the new provider: the switch resolves the
/// environment credential, exactly as a fresh spawn of the profile would.
#[tokio::test]
async fn provider_switch_without_a_realm_binding_uses_the_env_credential() {
    let config = global_config(&[(OPENAI_BINDING, Route::ApiKey(Provider::OpenAI), false)]);
    let harness = Harness::new(
        config,
        Scenario {
            env_key_present: true,
            ..Scenario::default()
        },
    );
    let durable = harness.create_openai_session(None).await;
    let mut resumed = expect_switched(harness.resume_switched(durable, None, None).await);
    resumed
        .run(SECOND_PROMPT.to_string().into())
        .await
        .expect("the next turn runs after the provider switch");
    let resolved = harness.log.resolved();
    assert!(
        !resolved.is_empty()
            && resolved
                .iter()
                .all(|(provider, binding)| *provider == Provider::Anthropic
                    && binding.is_env_default()),
        "the switch resolves the environment credential: {resolved:?}"
    );
    let identity = durable_identity(&resumed);
    assert_eq!(identity.provider, Provider::Anthropic);
    assert_eq!(
        identity.auth_binding, None,
        "an env-default route is not persisted as a binding"
    );
}

/// R6b. No realm binding and no environment credential: the existing typed
/// missing-credential refusal, never the old provider's binding.
#[tokio::test]
async fn provider_switch_without_any_credential_refuses_with_missing_secret() {
    let config = global_config(&[(OPENAI_BINDING, Route::ApiKey(Provider::OpenAI), false)]);
    let harness = Harness::new(config, Scenario::default());
    let durable = harness.create_openai_session(None).await;
    match harness.resume_switched(durable, None, None).await {
        Err(BuildAgentError::LlmClient(meerkat_client::FactoryError::ProviderAuth(
            ProviderAuthError::Auth(meerkat_core::AuthError::MissingSecret),
        ))) => {}
        Err(other) => panic!("expected the typed missing-secret refusal, got {other}"),
        Ok(_) => panic!("a switch with no credential for the new provider must refuse"),
    }
    assert_never_resolved_openai(&harness.log);
}

/// R7. A cold restart after the switch keeps the new binding: with no
/// override the durable Anthropic identity wins.
#[tokio::test]
async fn cold_restart_after_a_provider_switch_keeps_the_new_binding() {
    let harness = Harness::new(global_realm_config(), Scenario::default());
    let durable = harness.create_openai_session(None).await;
    let mut resumed = expect_switched(harness.resume_switched(durable, None, None).await);
    resumed
        .run(SECOND_PROMPT.to_string().into())
        .await
        .expect("the next turn runs after the provider switch");
    let switched = resumed.session().clone();
    drop(resumed);
    harness.log.clear();

    let mut restarted = harness
        .resume_unchanged(switched)
        .await
        .expect("an unchanged resume of the switched session builds");
    restarted
        .run(THIRD_PROMPT.to_string().into())
        .await
        .expect("the turn after the cold restart runs");
    assert_eq!(
        harness
            .log
            .served()
            .into_iter()
            .map(|(provider, model, _)| (provider, model))
            .collect::<Vec<_>>(),
        vec![(Provider::Anthropic, ANTHROPIC_MODEL.to_string())],
        "the cold restart keeps the switched identity"
    );
    assert_eq!(
        durable_identity(&restarted).auth_binding,
        Some(realm_binding(ANTHROPIC_BINDING))
    );
}

/// R7b (guard). A refused switch leaves the stored provider, model and
/// binding untouched, so a later resume without the override still works.
#[cfg(feature = "jsonl-store")]
#[tokio::test]
async fn refused_provider_switch_leaves_the_stored_identity_untouched() {
    let config = global_config(&[(OPENAI_BINDING, Route::ApiKey(Provider::OpenAI), false)]);
    let harness = Harness::new(config, Scenario::default());
    let durable = harness.create_openai_session(None).await;
    let session_id = durable.id().clone();
    let store = meerkat_store::JsonlStore::new(harness.store_path.clone());
    store
        .init()
        .await
        .expect("open the factory's session store");
    let before = stored_session(&store, &session_id)
        .await
        .session_metadata()
        .expect("stored metadata")
        .llm_identity();
    assert_eq!(before.provider, Provider::OpenAI);

    assert!(
        harness.resume_switched(durable, None, None).await.is_err(),
        "no credential serves Anthropic, so the switch refuses"
    );
    let stored = stored_session(&store, &session_id).await;
    assert_eq!(
        stored
            .session_metadata()
            .expect("stored metadata")
            .llm_identity(),
        before,
        "a refused switch writes nothing"
    );
    let mut resumed = harness
        .resume_unchanged(stored)
        .await
        .expect("a resume without the override still builds");
    resumed
        .run(SECOND_PROMPT.to_string().into())
        .await
        .expect("the session keeps working on OpenAI");
    assert_eq!(durable_identity(&resumed).provider, Provider::OpenAI);
}

#[cfg(feature = "jsonl-store")]
async fn stored_session(
    store: &meerkat_store::JsonlStore,
    session_id: &meerkat_core::SessionId,
) -> Session {
    use crate::SessionStore as _;
    store
        .load(session_id)
        .await
        .expect("read the stored session")
        .expect("the created session is stored")
}

/// R8. Cross-provider replay after the switch: the Anthropic request omits
/// the OpenAI reasoning item and replays the tool-use/tool-result pair intact.
#[cfg(feature = "anthropic")]
#[tokio::test]
async fn provider_switch_replays_tool_pairs_without_the_old_providers_reasoning() {
    let harness = Harness::new(
        global_realm_config(),
        Scenario {
            tool_round: true,
            ..Scenario::default()
        },
    );
    let durable = harness.create_openai_session(None).await;
    assert!(
        durable.messages().iter().any(|message| matches!(
            message,
            Message::BlockAssistant(assistant)
                if assistant.blocks.iter().any(|block| matches!(
                    block,
                    AssistantBlock::Reasoning { meta: Some(meta), .. }
                        if matches!(meta.as_ref(), meerkat_core::ProviderMeta::OpenAi { .. })
                ))
        )),
        "fixture: the OpenAI turn left an encrypted OpenAI reasoning item"
    );

    let mut resumed = expect_switched(harness.resume_switched(durable, None, None).await);
    resumed
        .run(SECOND_PROMPT.to_string().into())
        .await
        .expect("the next turn runs after the provider switch");
    let projected = harness
        .log
        .anthropic_projected()
        .into_iter()
        .next()
        .expect("Anthropic served the next turn");
    assert!(
        !projected.iter().any(|message| matches!(
            message,
            Message::BlockAssistant(assistant)
                if assistant
                    .blocks
                    .iter()
                    .any(|block| matches!(block, AssistantBlock::Reasoning { .. }))
        )),
        "the OpenAI reasoning item is not replayed to Anthropic: {projected:?}"
    );
    assert!(
        projected.iter().any(|message| matches!(
            message,
            Message::BlockAssistant(assistant)
                if assistant.blocks.iter().any(|block| matches!(
                    block,
                    AssistantBlock::ToolUse { id, .. } if id == ECHO_CALL
                ))
        )),
        "the tool use replays to Anthropic: {projected:?}"
    );
    assert!(
        projected.iter().any(|message| matches!(
            message,
            Message::ToolResults { results, .. }
                if results.iter().any(|result| result.tool_use_id == ECHO_CALL)
        )),
        "the tool result replays with its tool use: {projected:?}"
    );
}

/// A durable session written directly, for identities a fake runtime does
/// not build fresh.
fn durable_session(identity: SessionLlmIdentity) -> Session {
    let mut session = Session::new();
    session
        .set_session_metadata(meerkat_core::SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: identity.model,
            max_tokens: 4096,
            structured_output_retries: 2,
            provider: identity.provider,
            self_hosted_server_id: identity.self_hosted_server_id,
            provider_params: identity.provider_params,
            tooling: Default::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: None,
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: identity.auth_binding,
            mob_member_binding: None,
        })
        .expect("durable metadata");
    session
}

/// R13 (guard). Same provider whose durable binding no longer serves it
/// (config drift re-pointed the binding at another provider): the resume
/// keeps the durable binding and refuses strictly, with no silent fallback.
#[tokio::test]
async fn same_provider_resume_on_a_drifted_binding_stays_strict() {
    // `openai` now names an Anthropic backend and auth profile.
    let config = global_config(&[(OPENAI_BINDING, Route::ApiKey(Provider::Anthropic), false)]);
    let harness = Harness::new(config, Scenario::default());
    let durable = durable_session(SessionLlmIdentity {
        model: OPENAI_MODEL.to_string(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(realm_binding(OPENAI_BINDING)),
    });
    let mut resume = harness.build_config(OPENAI_MODEL_B, Provider::OpenAI);
    resume.resume_override_mask.model = true;
    resume.resume_override_mask.provider = true;
    resume.resume_session = Some(durable);
    match harness.factory.build_agent(resume, &harness.config).await {
        Err(BuildAgentError::LlmClient(meerkat_client::FactoryError::ConnectionTarget(
            meerkat_core::ConnectionTargetError::ProviderMismatch {
                binding,
                expected,
                backend,
                ..
            },
        ))) => {
            assert_eq!(
                (binding.as_str(), expected, backend),
                (OPENAI_BINDING, Provider::OpenAI, Provider::Anthropic)
            );
        }
        Err(other) => panic!("expected the strict ProviderMismatch, got {other}"),
        Ok(_) => panic!("a same-provider resume must not drop a drifted durable binding"),
    }
    assert!(harness.log.resolved().is_empty());
}

/// Two self-hosted servers, one alias each, and one binding per server in
/// `[realm.global]` (the muse binding is the realm default).
fn self_hosted_config() -> Config {
    use meerkat_core::config::{
        SelfHostedApiStyle, SelfHostedModelConfig, SelfHostedServerConfig, SelfHostedTransport,
    };
    let mut config = Config::default();
    config.self_hosted.default_model = Some("muse-glimmer-30b".to_string());
    let mut section = RealmConfigSection {
        default_binding: Some("muse".to_string()),
        ..Default::default()
    };
    for (server, model, binding, base_url) in [
        (
            "muse_vllm",
            "muse-glimmer-30b",
            "muse",
            "http://muse.invalid:8000",
        ),
        (
            "cerebras",
            "cerebras-gpt-oss-120b",
            "cerebras",
            "https://api.cerebras.invalid/v1",
        ),
    ] {
        config.self_hosted.servers.insert(
            server.to_string(),
            SelfHostedServerConfig {
                transport: SelfHostedTransport::OpenAiCompatible,
                base_url: base_url.to_string(),
                api_style: SelfHostedApiStyle::ChatCompletions,
            },
        );
        config.self_hosted.models.insert(
            model.to_string(),
            SelfHostedModelConfig {
                server: server.to_string(),
                remote_model: "remote".to_string(),
                ..Default::default()
            },
        );
        section.backend.insert(
            binding.to_string(),
            BackendProfileConfig {
                provider: "self_hosted".to_string(),
                backend_kind: "self_hosted".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: Some(server.to_string()),
            },
        );
        section.auth.insert(
            binding.to_string(),
            meerkat_core::AuthProfileConfig {
                provider: "self_hosted".to_string(),
                auth_method: "static_bearer".to_string(),
                source: CredentialSourceSpec::InlineSecret {
                    secret: format!("{binding}-secret"),
                },
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            binding.to_string(),
            ProviderBindingConfig {
                backend_profile: binding.to_string(),
                auth_profile: binding.to_string(),
                credential_account: None,
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        );
    }
    config.realm.insert(GLOBAL.to_string(), section);
    config
}

/// R10 (guard). A self-hosted server change under the mask keeps the
/// provider, so the durable server-scoped binding stays and the build refuses
/// typed, exactly as the turn-override seam keeps that binding.
#[cfg(feature = "openai")]
#[tokio::test]
async fn self_hosted_server_change_under_the_mask_refuses_typed_like_the_turn_seam() {
    let config = self_hosted_config();
    let harness = Harness::new(config.clone(), Scenario::default());
    let durable_identity = SessionLlmIdentity {
        model: "muse-glimmer-30b".to_string(),
        provider: Provider::SelfHosted,
        self_hosted_server_id: Some("muse_vllm".to_string()),
        provider_params: None,
        auth_binding: Some(realm_binding("muse")),
    };

    // The turn-override seam keeps the binding for a same-provider server
    // change.
    let registry = config
        .model_registry(meerkat_models::canonical())
        .expect("model registry");
    let turn_target = meerkat_core::resolve_session_llm_identity_override(
        &durable_identity,
        &registry,
        meerkat_core::SessionLlmIdentityOverride {
            model: Some("cerebras-gpt-oss-120b"),
            provider: Some(Provider::SelfHosted),
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
            reasoning_effort: None,
        },
    )
    .expect("the turn seam admits the server change");
    assert_eq!(turn_target.auth_binding, Some(realm_binding("muse")));

    let mut resume = harness.build_config("cerebras-gpt-oss-120b", Provider::SelfHosted);
    resume.resume_override_mask.model = true;
    resume.resume_override_mask.provider = true;
    resume.resume_session = Some(durable_session(durable_identity));
    match harness.factory.build_agent(resume, &harness.config).await {
        Err(BuildAgentError::LlmClient(meerkat_client::FactoryError::SelfHostedBinding(_))) => {}
        Err(other) => panic!("expected the typed SelfHostedBinding refusal, got {other}"),
        Ok(_) => panic!("a server-scoped binding must not authenticate another server"),
    }
}

/// R11 (factory half). The factory's resume merge seats the shared table's
/// identity for every case; the mob merge is checked against the same table.
#[test]
fn factory_resume_merge_matches_the_shared_identity_table() {
    let mut mismatches = Vec::new();
    for case in meerkat_core::resume_identity_cases::resume_identity_cases() {
        let mut build = AgentBuildConfig::new(case.candidate.model.clone());
        build.provider = Some(case.candidate.provider);
        build.self_hosted_server_id = case.candidate.self_hosted_server_id.clone();
        build.provider_params = case.candidate.provider_params.clone();
        build.auth_binding = case.candidate.auth_binding.clone();
        build.resume_override_mask = case.mask;
        build.resume_session = Some(durable_session(case.durable.clone()));
        AgentFactory::apply_resumed_session_metadata(&mut build).expect("durable metadata applies");
        let seated = SessionLlmIdentity {
            model: build.model.clone(),
            provider: build.provider.unwrap_or(case.durable.provider),
            self_hosted_server_id: build.self_hosted_server_id.clone(),
            provider_params: build.provider_params.clone(),
            auth_binding: build.auth_binding.clone(),
        };
        if seated != case.expected {
            mismatches.push(format!(
                "{}: seated {seated:?}, expected {:?}",
                case.name, case.expected
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "the factory merge departs from the shared resume identity table:\n{}",
        mismatches.join("\n")
    );
}

/// M5. The refused account switch stays typed across the session error
/// boundary, so the mob and Meta can classify it, and it is still a build
/// identity failure.
#[test]
fn credential_account_route_refusal_round_trips_as_a_typed_session_error() {
    let failure = meerkat_core::service::SessionCredentialAccountRouteFailure {
        provider: Provider::Anthropic,
        realm_id: RealmId::global(),
        account: meerkat_core::CredentialAccountId::parse(ACCOUNT).expect("valid account"),
    };
    let error =
        meerkat_core::service::SessionError::credential_account_route_failure(failure.clone());
    assert_eq!(error.credential_account_route_failure_data(), Some(failure));
    assert!(error.is_build_llm_identity_unresolvable());
    assert_eq!(error.provider_auth_failure_data(), None);
    assert!(
        error
            .to_string()
            .contains("has no route sharing credential account 'global:acct_a'"),
        "{error}"
    );
}
