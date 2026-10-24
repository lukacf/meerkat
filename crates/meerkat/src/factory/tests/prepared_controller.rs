//! A controller prepared before its session exists, under the session's
//! actual auth owner, and consumed exactly once by that session's build
//! (`crate::prepared_controller`): no second resolution, decoration,
//! selection, pin or model request at consumption; every changed controller
//! fact refused, typed; the retained adapters publishing to the consuming
//! build's own run channel and event tap.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::prepared_controller::{
    PreparedControllerCell, PreparedControllerDisposal, PreparedControllerRefusal,
    PreparedControllerTicket, encode_prepared_controller_for_service,
};
use meerkat_core::handles::{GeneratedAuthLeaseHandle, LeaseKey};
use meerkat_llm_core::provider_runtime::{
    ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
    ResolvedConnection, ResolvedTextTarget, ResolverEnvironment, StaticLease, ValidatedBinding,
};

const PRIMARY_MODEL: &str = "claude-sonnet-4-5";
const BACKUP_MODEL: &str = "custom-text-only-backup";
const REALM: &str = "prepared_controller_fallback";

/// Every resolution and client build of one provider, and the auth owner
/// each resolution ran under.
#[derive(Default)]
struct RouteRecord {
    resolves: AtomicUsize,
    builds: AtomicUsize,
    owners: std::sync::Mutex<Vec<GeneratedAuthLeaseHandle>>,
}

struct CountingRuntime {
    provider: Provider,
    client: Arc<dyn LlmClient>,
    record: Arc<RouteRecord>,
}

#[async_trait]
impl ProviderRuntime for CountingRuntime {
    fn provider_id(&self) -> Provider {
        self.provider
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        self.record.resolves.fetch_add(1, Ordering::SeqCst);
        if let Some(owner) = env.auth_lease_handle.clone() {
            self.record.owners.lock().unwrap().push(owner);
        }
        Ok(ResolvedConnection {
            provider: self.provider,
            backend: binding.backend(),
            backend_profile: Arc::clone(binding.backend_profile()),
            credential_identity: binding.credential_identity().clone(),
            auth_lease: Arc::new(StaticLease::inline_secret(
                format!("test-{}-key", self.provider.as_str()),
                meerkat_core::AuthMetadata::default(),
                None,
                format!("prepared-controller-test:{}", self.provider.as_str()),
            )),
        })
    }

    fn build_client(
        &self,
        _connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        self.record.builds.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::clone(&self.client))
    }
}

/// Counts the live wrappers a decorator made: one per decorated client.
struct LiveWrapper(Arc<AtomicUsize>);

impl LiveWrapper {
    fn new(live: &Arc<AtomicUsize>) -> Self {
        live.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(live))
    }
}

impl Drop for LiveWrapper {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A final-client decorator that forwards everything, the controller pin
/// included, and counts itself.
struct CountingFinalClient {
    inner: Arc<dyn AgentLlmClient>,
    _live: LiveWrapper,
}

#[async_trait]
impl AgentLlmClient for CountingFinalClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    fn controller_model_facts(
        &self,
    ) -> Result<meerkat_core::ControllerModelFacts, meerkat_core::ControllerFactsUnavailable> {
        self.inner.controller_model_facts()
    }

    fn pin_controller(self: Arc<Self>) -> Option<meerkat_core::ControllerModelClient> {
        let selection = self.controller_model_selection()?;
        Some(meerkat_core::ControllerModelClient::new(selection, self))
    }

    fn prepare_request_attempt(
        self: Arc<Self>,
        messages: Arc<Vec<meerkat_core::Message>>,
        tools: Arc<[Arc<meerkat_core::ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<Arc<dyn meerkat_core::AgentLlmRequestAttempt>, meerkat_core::AgentError> {
        Arc::clone(&self.inner).prepare_request_attempt(
            messages,
            tools,
            max_tokens,
            temperature,
            provider_params,
        )
    }

    fn request_attempt_authority(&self) -> meerkat_core::RequestAttemptAuthority {
        self.inner.request_attempt_authority()
    }

    fn prepare_request_attempt_authorized(
        self: Arc<Self>,
        messages: Arc<Vec<meerkat_core::Message>>,
        tools: Arc<[Arc<meerkat_core::ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<Arc<dyn meerkat_core::AgentLlmRequestAttempt>, meerkat_core::AgentError> {
        Arc::clone(&self.inner).prepare_request_attempt_authorized(
            messages,
            tools,
            max_tokens,
            temperature,
            provider_params,
            authorization,
        )
    }

    async fn stream_response_authorized(
        &self,
        messages: &[meerkat_core::Message],
        tools: &[Arc<meerkat_core::ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
        authorization: Option<meerkat_core::LlmRequestAuthorization>,
    ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
        self.inner
            .stream_response_authorized(
                messages,
                tools,
                max_tokens,
                temperature,
                provider_params,
                authorization,
            )
            .await
    }

    fn request_pressure(
        &self,
        messages: &[meerkat_core::Message],
        tools: &[Arc<meerkat_core::ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, meerkat_core::AgentError> {
        self.inner
            .request_pressure(messages, tools, max_tokens, temperature, provider_params)
    }

    fn target_cache_lowering_capabilities(
        &self,
        issuer: &meerkat_core::TargetCacheLoweringIssuer,
        messages: &[meerkat_core::Message],
        tools: &[Arc<meerkat_core::ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<Vec<meerkat_core::TargetCacheLoweringCapability>, meerkat_core::AgentError> {
        self.inner.target_cache_lowering_capabilities(
            issuer,
            messages,
            tools,
            max_tokens,
            temperature,
            provider_params,
        )
    }

    fn begin_stream_output_observation(&self) {
        self.inner.begin_stream_output_observation();
    }

    fn stream_output_observed(&self) -> bool {
        self.inner.stream_output_observed()
    }

    fn stream_activity_count(&self) -> Option<u64> {
        self.inner.stream_activity_count()
    }

    async fn stream_response(
        &self,
        messages: &[meerkat_core::Message],
        tools: &[Arc<meerkat_core::ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
        self.inner
            .stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await
    }

    fn provider(&self) -> Provider {
        self.inner.provider()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn compile_schema(
        &self,
        output_schema: &OutputSchema,
    ) -> Result<meerkat_core::CompiledSchema, meerkat_core::SchemaError> {
        self.inner.compile_schema(output_schema)
    }
}

/// External tools whose update poll never finishes: a build that waits for
/// its tool connections stops there, after its controller was taken.
struct HeldUpdates {
    entered: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[async_trait]
impl AgentToolDispatcher for HeldUpdates {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        Err(meerkat_core::ToolError::not_found(call.name))
    }

    async fn poll_external_updates(&self) -> meerkat_core::ExternalToolUpdate {
        if let Some(entered) = self.entered.lock().unwrap().take() {
            let _ = entered.send(());
        }
        std::future::pending().await
    }
}

/// The facts a test observes: route resolutions and builds, final-client
/// decorations and model requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Observed {
    primary_resolves: usize,
    backup_resolves: usize,
    primary_builds: usize,
    backup_builds: usize,
    decorations: usize,
    primary_requests: usize,
    backup_requests: usize,
}

/// A primary that fails recoverably and a configured fallback that answers,
/// both resolved by counting runtimes, with a counting final-client
/// decorator, for sessions of one ephemeral machine.
struct Fixture {
    temp: tempfile::TempDir,
    factory: AgentFactory,
    config: Config,
    primary: Arc<RouteRecord>,
    backup: Arc<RouteRecord>,
    primary_requests: Arc<AtomicUsize>,
    backup_requests: Arc<AtomicUsize>,
    decorations: Arc<AtomicUsize>,
    live_wrappers: Arc<AtomicUsize>,
    decorator: AgentLlmClientDecorator,
    runtime: meerkat_runtime::MeerkatMachine,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let primary = Arc::new(RouteRecord::default());
        let backup = Arc::new(RouteRecord::default());
        let primary_requests = Arc::new(AtomicUsize::new(0));
        let backup_requests = Arc::new(AtomicUsize::new(0));
        let mut factory = AgentFactory::new(temp.path().join("sessions"))
            .without_provider_auth_persistence()
            .builtins(false);
        factory.provider_registry = Arc::new(
            ProviderRuntimeRegistry::empty()
                .with_runtime(Arc::new(CountingRuntime {
                    provider: Provider::Anthropic,
                    client: Arc::new(ScriptedStickyFallbackClient {
                        provider: Provider::Anthropic,
                        fail_recoverably: true,
                        calls: Arc::clone(&primary_requests),
                    }),
                    record: Arc::clone(&primary),
                }))
                .with_runtime(Arc::new(CountingRuntime {
                    provider: Provider::OpenAI,
                    client: Arc::new(ScriptedStickyFallbackClient {
                        provider: Provider::OpenAI,
                        fail_recoverably: false,
                        calls: Arc::clone(&backup_requests),
                    }),
                    record: Arc::clone(&backup),
                })),
        );
        let decorations = Arc::new(AtomicUsize::new(0));
        let live_wrappers = Arc::new(AtomicUsize::new(0));
        let decorator: AgentLlmClientDecorator = {
            let decorations = Arc::clone(&decorations);
            let live_wrappers = Arc::clone(&live_wrappers);
            Arc::new(move |inner| {
                decorations.fetch_add(1, Ordering::SeqCst);
                Arc::new(CountingFinalClient {
                    inner,
                    _live: LiveWrapper::new(&live_wrappers),
                })
            })
        };
        Self {
            temp,
            factory,
            config: fallback_config(),
            primary,
            backup,
            primary_requests,
            backup_requests,
            decorations,
            live_wrappers,
            decorator,
            runtime: meerkat_runtime::MeerkatMachine::ephemeral(),
        }
    }

    fn observed(&self) -> Observed {
        Observed {
            primary_resolves: self.primary.resolves.load(Ordering::SeqCst),
            backup_resolves: self.backup.resolves.load(Ordering::SeqCst),
            primary_builds: self.primary.builds.load(Ordering::SeqCst),
            backup_builds: self.backup.builds.load(Ordering::SeqCst),
            decorations: self.decorations.load(Ordering::SeqCst),
            primary_requests: self.primary_requests.load(Ordering::SeqCst),
            backup_requests: self.backup_requests.load(Ordering::SeqCst),
        }
    }

    /// The session's build configuration, as its request makes it.
    fn request(&self) -> AgentBuildConfig {
        let mut build = AgentBuildConfig::new(PRIMARY_MODEL);
        build.max_tokens = Some(1024);
        build.provider = Some(Provider::Anthropic);
        build.realm_id = Some(RealmId::parse(REALM).unwrap());
        build.auth_binding = Some(configured_auth_binding(REALM, "default_anthropic"));
        build.override_builtins = ToolCategoryOverride::Disable;
        // Image-generation executors resolve image bindings of their own;
        // the counts here are the controller's alone.
        build.override_image_generation = ToolCategoryOverride::Disable;
        build.agent_llm_client_decorator = Some(Arc::clone(&self.decorator));
        build
    }

    async fn prepare(&self, session_id: &SessionId) -> PreparedControllerTicket {
        let graph = self
            .factory
            .prepare_controller_graph(
                self.request(),
                &self.config,
                self.runtime.generated_auth_lease_handle(),
                session_id,
            )
            .await
            .expect("the controller is prepared under the session's auth owner");
        PreparedControllerTicket::new(PreparedControllerCell::new(session_id.clone(), graph))
    }

    /// The session's own build, minting its pre-assigned session with
    /// `bindings` and consuming `ticket`.
    fn consuming_build(
        &self,
        ticket: &PreparedControllerTicket,
        bindings: meerkat_core::SessionRuntimeBindings,
        event_tx: Option<mpsc::Sender<AgentEvent>>,
    ) -> AgentBuildConfig {
        let mut build = self.request();
        build.resume_session = Some(Session::with_id(ticket.session_id().clone()));
        build.resume_session_intent = meerkat_core::SessionBuildIntent::Mint;
        build.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings);
        build.prepared_controller = Some(encode_prepared_controller_for_service(ticket.clone()));
        build.event_tx = event_tx;
        build
    }
}

fn fallback_config() -> Config {
    let mut config = Config {
        max_tokens: Some(1024),
        ..Default::default()
    };
    config.realm.insert(
        REALM.to_string(),
        inline_realm_section(&[
            ("anthropic", "test-primary-key"),
            ("openai", "test-backup-key"),
        ]),
    );
    config.models.custom.insert(
        BACKUP_MODEL.to_string(),
        meerkat_core::config::CustomModelConfig {
            provider: Provider::OpenAI,
            display_name: Some("Text-only backup".to_string()),
            context_window: Some(32_000),
            max_input_tokens: None,
            max_output_tokens: Some(4_096),
            vision: Some(false),
            web_search: Some(false),
            call_timeout_secs: None,
        },
    );
    config.model_fallback.enabled = Some(true);
    config.model_fallback.policy.cross_provider = true;
    config.model_fallback.policy.trigger_after_attempts = 1;
    config.model_fallback.policy.require_tool_parity = false;
    config.model_fallback.chain = vec![ModelFallbackTarget {
        model: BACKUP_MODEL.to_string(),
        provider: Some(Provider::OpenAI),
        auth_binding: Some(configured_auth_binding(REALM, "default_openai")),
    }];
    config.retry.max_retries = 1;
    config.retry.initial_delay = std::time::Duration::ZERO;
    config.retry.max_delay = std::time::Duration::ZERO;
    config.retry.multiplier = 1.0;
    config
}

fn refusal(result: Result<DynAgent, BuildAgentError>) -> Option<PreparedControllerRefusal> {
    match result {
        Ok(_) => None,
        Err(BuildAgentError::PreparedController(refusal)) => Some(refusal),
        Err(other) => panic!("expected a prepared-controller outcome, got {other}"),
    }
}

fn is_backup_delta(event: &AgentEvent) -> bool {
    matches!(event, AgentEvent::TextDelta { delta, .. } if delta == "backup success")
}

/// Preparation resolves, decorates, assembles the fallback and pins under
/// the session's auth owner without a model request. The session's build
/// takes that exact graph without resolving, building, decorating or
/// requesting anything, and its turn runs on the retained primary and
/// fallback, whose adapters publish to the build's own run channel and to
/// the agent's own event tap.
#[tokio::test]
async fn a_prepared_controller_is_consumed_without_resolution_decoration_or_requests() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let ticket = fixture.prepare(&session_id).await;
    let prepared = fixture.observed();
    assert_eq!(
        prepared,
        Observed {
            primary_resolves: 1,
            backup_resolves: 1,
            primary_builds: 1,
            backup_builds: 1,
            decorations: 2,
            primary_requests: 0,
            backup_requests: 0,
        },
        "preparation assembles the whole graph and sends no request"
    );
    assert_eq!(fixture.live_wrappers.load(Ordering::SeqCst), 2);
    let owner = fixture.runtime.generated_auth_lease_handle();
    for record in [&fixture.primary, &fixture.backup] {
        let owners = record.owners.lock().unwrap();
        assert!(
            !owners.is_empty() && owners.iter().all(|seen| seen.same_owner(&owner)),
            "every resolution ran under the session's own auth owner"
        );
    }

    let bindings = fixture
        .runtime
        .prepare_bindings(session_id.clone())
        .await
        .unwrap();
    assert!(bindings.auth_lease().same_owner(&owner));
    let (event_tx, mut event_rx) = mpsc::channel(256);
    let mut agent = fixture
        .factory
        .build_agent(
            fixture.consuming_build(&ticket, bindings, Some(event_tx)),
            &fixture.config,
        )
        .await
        .expect("the session's build consumes the prepared controller");
    assert_eq!(
        fixture.observed(),
        prepared,
        "consumption resolves, builds, decorates and requests nothing"
    );

    let (tap_tx, mut tap_rx) = mpsc::channel(256);
    *agent.event_tap().lock() = Some(meerkat_core::EventTapState {
        tx: tap_tx,
        truncated: AtomicBool::new(false),
    });
    agent.set_runtime_execution_kind(Some(
        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
    ));
    let result = agent
        .run(meerkat_core::ContentInput::Text(
            "exercise the retained fallback".to_string(),
        ))
        .await
        .expect("the retained fallback answers");
    assert_eq!(result.text, "backup success");
    assert_eq!(
        fixture.observed(),
        Observed {
            primary_requests: 1,
            backup_requests: 1,
            ..prepared
        },
        "the turn ran on the retained primary and fallback, nothing rebuilt"
    );
    let channel: Vec<AgentEvent> = std::iter::from_fn(|| event_rx.try_recv().ok()).collect();
    assert_eq!(
        channel
            .iter()
            .filter(|event| is_backup_delta(event))
            .count(),
        1,
        "the retained fallback adapter publishes to the build's run channel"
    );
    let tapped: Vec<AgentEvent> = std::iter::from_fn(|| tap_rx.try_recv().ok()).collect();
    assert_eq!(
        tapped.iter().filter(|event| is_backup_delta(event)).count(),
        1,
        "and to the agent's own event tap: the one tap of the retained graph"
    );
    assert_eq!(
        ticket.cell().take_if(|_| Ok(())).err(),
        Some(PreparedControllerRefusal::Consumed)
    );
}

/// Each controller fact that changed after preparation (a custom model
/// definition, a realm's binding or backend, the fallback configuration,
/// the final-client decorator) refuses the build, typed, with nothing
/// resolved in its place, and leaves the prepared controller ready. An
/// unrelated configuration change refuses nothing: that build takes it.
#[tokio::test]
async fn changed_controller_facts_refuse_and_unrelated_configuration_consumes() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let ticket = fixture.prepare(&session_id).await;
    let prepared = fixture.observed();
    let bindings = fixture
        .runtime
        .prepare_bindings(session_id.clone())
        .await
        .unwrap();
    type Change = Box<dyn Fn(&mut Config, &mut AgentBuildConfig)>;
    let changes: Vec<(&str, Change)> = vec![
        (
            "configured custom models",
            Box::new(|config, _| {
                config
                    .models
                    .custom
                    .get_mut(BACKUP_MODEL)
                    .unwrap()
                    .context_window = Some(16_000);
            }),
        ),
        (
            "realm connections",
            Box::new(|config, _| {
                let section = config.realm.get_mut(REALM).unwrap();
                let mut changed = 0;
                for backend in section
                    .backend
                    .values_mut()
                    .filter(|backend| backend.provider == "anthropic")
                {
                    backend.base_url = Some("https://changed.example/v1".to_string());
                    changed += 1;
                }
                assert!(changed > 0, "the primary's backend is configured");
            }),
        ),
        (
            "model fallback",
            Box::new(|config, _| {
                config.model_fallback.policy.trigger_after_attempts = 2;
            }),
        ),
        (
            "final client decorator",
            Box::new(|_, build| {
                build.agent_llm_client_decorator = Some(Arc::new(|client| client));
            }),
        ),
    ];
    for (fact, change) in changes {
        let mut config = fixture.config.clone();
        let mut build = fixture.consuming_build(&ticket, bindings.clone(), None);
        change(&mut config, &mut build);
        assert_eq!(
            refusal(fixture.factory.build_agent(build, &config).await),
            Some(PreparedControllerRefusal::InputsChanged(fact)),
            "{fact}"
        );
        assert_eq!(
            fixture.observed(),
            prepared,
            "{fact}: nothing resolved in its place"
        );
    }

    // The output budget and the retry policy select, define and shape no
    // route of the controller.
    let mut config = fixture.config.clone();
    config.max_tokens = Some(2048);
    config.retry.max_retries = 3;
    assert_eq!(
        refusal(
            fixture
                .factory
                .build_agent(fixture.consuming_build(&ticket, bindings, None), &config)
                .await
        ),
        None,
        "an unrelated configuration change consumes the prepared controller"
    );
    assert_eq!(fixture.observed(), prepared);
}

/// A prepared controller is taken once. A foreign payload, an injected
/// client, a standalone build, another session and another auth owner are
/// refused, typed, and leave it ready; of two cloned tickets racing, exactly
/// one build takes it and the other is refused as consumed; a retired cell
/// refuses every alias.
#[tokio::test]
async fn a_prepared_controller_is_taken_once_and_refuses_every_other_build() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let ticket = fixture.prepare(&session_id).await;
    let bindings = fixture
        .runtime
        .prepare_bindings(session_id.clone())
        .await
        .unwrap();
    let build =
        |ticket: &PreparedControllerTicket| fixture.consuming_build(ticket, bindings.clone(), None);

    let mut foreign = build(&ticket);
    foreign.prepared_controller = Some(Arc::new(7_u32));
    assert_eq!(
        refusal(fixture.factory.build_agent(foreign, &fixture.config).await),
        Some(PreparedControllerRefusal::ForeignPayload)
    );

    let mut injected = build(&ticket);
    injected.llm_client_override = Some(Arc::new(ScriptedStickyFallbackClient {
        provider: Provider::Anthropic,
        fail_recoverably: false,
        calls: Arc::new(AtomicUsize::new(0)),
    }));
    assert_eq!(
        refusal(fixture.factory.build_agent(injected, &fixture.config).await),
        Some(PreparedControllerRefusal::InjectedClient)
    );

    let mut standalone = build(&ticket);
    standalone.resume_session = None;
    standalone.runtime_build_mode = RuntimeBuildMode::StandaloneEphemeral;
    assert_eq!(
        refusal(
            fixture
                .factory
                .build_agent(standalone, &fixture.config)
                .await
        ),
        Some(PreparedControllerRefusal::NotSessionOwned)
    );

    let other_session = SessionId::new();
    let mut elsewhere = fixture.consuming_build(
        &ticket,
        fixture
            .runtime
            .prepare_bindings(other_session.clone())
            .await
            .unwrap(),
        None,
    );
    elsewhere.resume_session = Some(Session::with_id(other_session));
    assert_eq!(
        refusal(
            fixture
                .factory
                .build_agent(elsewhere, &fixture.config)
                .await
        ),
        Some(PreparedControllerRefusal::SessionMismatch)
    );

    let other_owner = meerkat_runtime::MeerkatMachine::ephemeral();
    let foreign_owner = fixture.consuming_build(
        &ticket,
        other_owner
            .prepare_bindings(session_id.clone())
            .await
            .unwrap(),
        None,
    );
    assert_eq!(
        refusal(
            fixture
                .factory
                .build_agent(foreign_owner, &fixture.config)
                .await
        ),
        Some(PreparedControllerRefusal::AuthOwnerMismatch)
    );

    let (first, second) = tokio::join!(
        Box::pin(
            fixture
                .factory
                .build_agent(build(&ticket.clone()), &fixture.config)
        ),
        Box::pin(
            fixture
                .factory
                .build_agent(build(&ticket.clone()), &fixture.config)
        ),
    );
    let outcomes = [refusal(first), refusal(second)];
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_none()).count(),
        1,
        "exactly one build takes it: {outcomes:?}"
    );
    assert!(
        outcomes.contains(&Some(PreparedControllerRefusal::Consumed)),
        "{outcomes:?}"
    );

    let retired_session = SessionId::new();
    let retired = fixture.prepare(&retired_session).await;
    let alias = retired.clone();
    assert_eq!(retired.cell().retire(), PreparedControllerDisposal::Drained);
    assert_eq!(
        refusal(
            fixture
                .factory
                .build_agent(
                    fixture.consuming_build(
                        &alias,
                        fixture
                            .runtime
                            .prepare_bindings(retired_session)
                            .await
                            .unwrap(),
                        None,
                    ),
                    &fixture.config,
                )
                .await
        ),
        Some(PreparedControllerRefusal::Retired)
    );
}

/// A build cancelled after it took the prepared controller drops the
/// retained graph with it: every retained client is released, and every
/// alias is refused as consumed.
#[tokio::test]
async fn a_build_cancelled_after_the_take_drops_the_retained_graph() {
    let fixture = Fixture::new();
    let session_id = SessionId::new();
    let ticket = fixture.prepare(&session_id).await;
    assert_eq!(fixture.live_wrappers.load(Ordering::SeqCst), 2);
    let bindings = fixture
        .runtime
        .prepare_bindings(session_id.clone())
        .await
        .unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let mut build = fixture.consuming_build(&ticket, bindings.clone(), None);
    build.external_tools = Some(Arc::new(HeldUpdates {
        entered: std::sync::Mutex::new(Some(entered_tx)),
    }));
    build.wait_for_mcp = true;
    let factory = fixture.factory.clone();
    let config = fixture.config.clone();
    let task = tokio::spawn(async move { factory.build_agent(build, &config).await.map(drop) });
    entered_rx
        .await
        .expect("the build waits on its tool connections after the take");
    assert_eq!(
        ticket.cell().take_if(|_| Ok(())).err(),
        Some(PreparedControllerRefusal::Consumed),
        "the build took the prepared controller"
    );
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(
        fixture.live_wrappers.load(Ordering::SeqCst),
        0,
        "the cancelled build released every retained client"
    );
    assert_eq!(
        refusal(
            fixture
                .factory
                .build_agent(
                    fixture.consuming_build(&ticket, bindings, None),
                    &fixture.config
                )
                .await
        ),
        Some(PreparedControllerRefusal::Consumed)
    );
}

/// Through the actual persistent service: the controller prepared under the
/// service machine's auth owner is consumed by the session the service
/// materializes under the pre-assigned id, with nothing resolved again, and
/// the retained fallback's output reaches the session's own event stream
/// (the channel the service supplies at build).
#[cfg(feature = "session-store")]
#[tokio::test]
async fn the_persistent_service_streams_the_retained_fallback_to_its_session() {
    use futures::StreamExt as _;

    let fixture = Fixture::new();
    let builder = crate::FactoryAgentBuilder::new(fixture.factory.clone(), fixture.config.clone());
    *builder
        .default_agent_llm_client_decorator
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&fixture.decorator));
    let persistence = crate::PersistenceBundle::new(
        Arc::new(meerkat_store::JsonlStore::new(
            fixture.temp.path().join("jsonl"),
        )) as Arc<dyn crate::SessionStore>,
        Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(
                fixture.temp.path().join("runtime.sqlite3"),
            )
            .unwrap(),
        ),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    )
    .unwrap();
    let (service, adapter) =
        crate::surface::build_runtime_backed_service_with_default_reconfigure_host(
            builder.clone(),
            4,
            persistence,
            fixture.temp.path().join("config_state.json"),
        )
        .expect("compose the runtime-backed service");

    let session_id = SessionId::new();
    let mut request = CreateSessionRequest {
        injected_context: Vec::new(),
        model: PRIMARY_MODEL.to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        system_prompt: crate::SystemPromptOverride::Set("prepared controller".to_string()),
        max_tokens: Some(1024),
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions {
            provider: Some(Provider::Anthropic),
            realm_id: Some(RealmId::parse(REALM).unwrap()),
            auth_binding: Some(configured_auth_binding(REALM, "default_anthropic")),
            override_builtins: ToolCategoryOverride::Disable,
            override_image_generation: ToolCategoryOverride::Disable,
            ..Default::default()
        }),
        labels: None,
    };
    let graph = builder
        .prepare_controller_graph(&request, adapter.generated_auth_lease_handle(), &session_id)
        .await
        .expect("prepared under the service machine's auth owner");
    let ticket =
        PreparedControllerTicket::new(PreparedControllerCell::new(session_id.clone(), graph));
    let prepared = fixture.observed();
    ticket.seat_build(request.build.get_or_insert_with(Default::default));
    Box::pin(crate::surface::materialize_session(
        &service,
        &adapter,
        Session::with_id(session_id.clone()),
        request,
        {
            let service = Arc::clone(&service);
            let adapter = Arc::clone(&adapter);
            move |session_id| {
                crate::surface::default_persistent_executor(service, adapter, session_id)
            }
        },
    ))
    .await
    .expect("the service materializes the session with the prepared controller");
    assert_eq!(
        fixture.observed(),
        prepared,
        "materialization resolved nothing again"
    );

    let mut events = service.subscribe_session_events(&session_id).await.unwrap();
    let (_, completion) = adapter
        .accept_input_with_completion(
            &session_id,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                "exercise the retained fallback",
                None,
            )),
        )
        .await
        .expect("the runtime accepts the prompt");
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        completion.expect("a completion handle").wait(),
    )
    .await
    .expect("the turn settles in time")
    .expect("the completion resolves");
    assert!(
        matches!(
            outcome,
            meerkat_runtime::completion::CompletionOutcome::Completed(ref run)
                if run.text == "backup success"
        ),
        "{outcome:?}"
    );
    let delivered = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(envelope) = events.next().await {
            if is_backup_delta(&envelope.payload) {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(
        delivered,
        "the retained fallback adapter publishes to the session's own event stream"
    );
    assert_eq!(
        fixture.observed(),
        Observed {
            primary_requests: 1,
            backup_requests: 1,
            ..prepared
        }
    );
}

/// A managed-store account binding resolved under the session's auth owner
/// publishes its credential into that owner, and the client the binding
/// resolved consults that same owner at request time (as lazy authorizers
/// do): the session's build takes the retained client and its turn finds the
/// credential through the bindings' own owner, with no resolution at
/// consumption.
#[tokio::test]
async fn a_managed_lazy_credential_is_published_into_and_consulted_on_the_session_owner() {
    let temp = tempfile::tempdir().unwrap();
    let observed = Arc::new(ManagedLease::default());
    let mut factory = AgentFactory::new(temp.path().join("sessions"))
        .without_provider_auth_persistence()
        .builtins(false);
    factory.provider_registry = Arc::new(ProviderRuntimeRegistry::empty().with_runtime(Arc::new(
        ManagedLeaseRuntime {
            observed: Arc::clone(&observed),
        },
    )));
    let config = managed_config();
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let mut request = AgentBuildConfig::new(MANAGED_MODEL);
    request.provider = Some(Provider::OpenAI);
    request.realm_id = Some(RealmId::parse(MANAGED_REALM).unwrap());
    request.auth_binding = Some(managed_binding());
    request.override_builtins = ToolCategoryOverride::Disable;
    request.override_image_generation = ToolCategoryOverride::Disable;
    let graph = factory
        .prepare_controller_graph(
            request.clone(),
            &config,
            runtime.generated_auth_lease_handle(),
            &session_id,
        )
        .await
        .expect("the managed binding resolves under the session's auth owner");
    let ticket =
        PreparedControllerTicket::new(PreparedControllerCell::new(session_id.clone(), graph));
    assert_eq!(observed.resolves.load(Ordering::SeqCst), 1);

    let bindings = runtime.prepare_bindings(session_id.clone()).await.unwrap();
    {
        let lease = observed.lease.lock().unwrap();
        let (owner, credential) = lease.as_ref().expect("resolved with the auth owner");
        assert!(owner.same_owner(bindings.auth_lease()));
        assert!(
            bindings
                .auth_lease()
                .snapshot(&LeaseKey::from_credential_identity(credential))
                .credential_present,
            "the resolution's publication landed in the owner the bindings clone"
        );
    }
    let mut build = request;
    build.resume_session = Some(Session::with_id(session_id));
    build.resume_session_intent = meerkat_core::SessionBuildIntent::Mint;
    build.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings);
    build.prepared_controller = Some(encode_prepared_controller_for_service(ticket));
    let (event_tx, mut event_rx) = mpsc::channel(256);
    build.event_tx = Some(event_tx);
    let mut agent = factory
        .build_agent(build, &config)
        .await
        .expect("the session's build consumes the managed controller");
    let (tap_tx, mut tap_rx) = mpsc::channel(256);
    *agent.event_tap().lock() = Some(meerkat_core::EventTapState {
        tx: tap_tx,
        truncated: AtomicBool::new(false),
    });
    agent.set_runtime_execution_kind(Some(
        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
    ));
    let result = agent
        .run(meerkat_core::ContentInput::Text("answer".to_string()))
        .await
        .expect("the retained client finds its credential on the session's owner");
    assert_eq!(result.text, MANAGED_ANSWER);
    assert_eq!(
        observed.resolves.load(Ordering::SeqCst),
        1,
        "consumption resolved nothing"
    );
    // The retained primary adapter publishes to the build's channel and the
    // agent's tap, as the retained fallback does in the fallback scenario.
    let answered = |event: &AgentEvent| matches!(event, AgentEvent::TextDelta { delta, .. } if delta == MANAGED_ANSWER);
    let channel: Vec<AgentEvent> = std::iter::from_fn(|| event_rx.try_recv().ok()).collect();
    assert_eq!(channel.iter().filter(|event| answered(event)).count(), 1);
    let tapped: Vec<AgentEvent> = std::iter::from_fn(|| tap_rx.try_recv().ok()).collect();
    assert_eq!(tapped.iter().filter(|event| answered(event)).count(), 1);
}

/// A self-hosted controller prepared under the session's auth owner
/// publishes its resolved lease into it; the session's build takes it with
/// no second resolution.
#[cfg(feature = "openai")]
#[tokio::test]
async fn a_self_hosted_controller_is_published_into_and_consumed_by_the_session_owner() {
    let temp = tempfile::tempdir().unwrap();
    let mut factory = AgentFactory::new(temp.path().join("sessions")).builtins(false);
    let calls = install_recording_self_hosted_runtime(&mut factory);
    let mut config = config_with_self_hosted_server(SelfHostedServerConfig {
        transport: SelfHostedTransport::OpenAiCompatible,
        base_url: "http://127.0.0.1:11434".to_string(),
        api_style: SelfHostedApiStyle::ChatCompletions,
    });
    insert_self_hosted_realm_binding(
        &mut config,
        "global",
        "default_local",
        "http://realm.example/v1",
        CredentialSourceSpec::InlineSecret {
            secret: "realm-token".to_string(),
        },
    );
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let mut request = AgentBuildConfig::new("gemma-4-e2b");
    request.provider = Some(Provider::SelfHosted);
    request.override_builtins = ToolCategoryOverride::Disable;
    let graph = factory
        .prepare_controller_graph(
            request.clone(),
            &config,
            runtime.generated_auth_lease_handle(),
            &session_id,
        )
        .await
        .expect("the self-hosted controller is prepared");
    assert_eq!(graph.resolved_identity().provider, Provider::SelfHosted);
    let ticket =
        PreparedControllerTicket::new(PreparedControllerCell::new(session_id.clone(), graph));
    let lease_key = {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].auth_lease_handle_present);
        LeaseKey::from_auth_binding(&calls[0].auth_binding)
    };

    let bindings = runtime.prepare_bindings(session_id.clone()).await.unwrap();
    assert_eq!(
        bindings.auth_lease().snapshot(&lease_key).phase,
        Some(meerkat_core::handles::AuthLeasePhase::Valid),
        "the publication landed in the owner the session's bindings clone"
    );
    let mut build = request;
    build.resume_session = Some(Session::with_id(session_id));
    build.resume_session_intent = meerkat_core::SessionBuildIntent::Mint;
    build.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings);
    build.prepared_controller = Some(encode_prepared_controller_for_service(ticket));
    factory
        .build_agent(build, &config)
        .await
        .expect("the session's build consumes the self-hosted controller");
    assert_eq!(
        calls.lock().unwrap().len(),
        1,
        "consumption resolved nothing"
    );
}

const MANAGED_MODEL: &str = "gpt-5.4";
const MANAGED_REALM: &str = "default";
const MANAGED_BINDING: &str = "default_openai";
const MANAGED_ACCOUNT: &str = "account_a";
const MANAGED_ANSWER: &str = "answered";

fn managed_binding() -> AuthBindingRef {
    configured_auth_binding(MANAGED_REALM, MANAGED_BINDING)
}

/// A managed-store account binding (as a Copilot account is configured).
fn managed_config() -> Config {
    let mut config = Config::default();
    let mut section = RealmConfigSection::default();
    section.backend.insert(
        MANAGED_BINDING.to_string(),
        BackendProfileConfig {
            provider: Provider::OpenAI.as_str().to_string(),
            backend_kind: "copilot".to_string(),
            base_url: None,
            options: serde_json::Value::Null,
            server: None,
        },
    );
    section.auth.insert(
        MANAGED_BINDING.to_string(),
        AuthProfileConfig {
            provider: Provider::OpenAI.as_str().to_string(),
            auth_method: "github_copilot_oauth".to_string(),
            source: CredentialSourceSpec::ManagedStore,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    section.binding.insert(
        MANAGED_BINDING.to_string(),
        ProviderBindingConfig {
            backend_profile: MANAGED_BINDING.to_string(),
            auth_profile: MANAGED_BINDING.to_string(),
            credential_account: Some(
                meerkat_core::CredentialAccountId::parse(MANAGED_ACCOUNT).unwrap(),
            ),
            default_model: None,
            policy: Default::default(),
            provider_default: true,
        },
    );
    config.realm.insert(MANAGED_REALM.to_string(), section);
    config
}

/// The auth owner and credential the managed binding resolved with.
#[derive(Default)]
struct ManagedLease {
    resolves: AtomicUsize,
    lease: std::sync::Mutex<
        Option<(
            GeneratedAuthLeaseHandle,
            meerkat_core::AuthCredentialIdentity,
        )>,
    >,
}

impl ManagedLease {
    /// Whether the resolved credential is present on the owner it resolved
    /// under, read at request time.
    fn present(&self) -> Result<(), String> {
        let lease = self.lease.lock().unwrap();
        let (owner, credential) = lease
            .as_ref()
            .ok_or_else(|| "no owner-backed resolution happened".to_string())?;
        if owner
            .snapshot(&LeaseKey::from_credential_identity(credential))
            .credential_present
        {
            Ok(())
        } else {
            Err(format!("no credential on the owner for {credential:?}"))
        }
    }
}

/// Answers only while its resolved credential is present on the owner it
/// resolved under, consulted at request time.
struct ManagedLeaseClient {
    observed: Arc<ManagedLease>,
}

#[async_trait]
impl LlmClient for ManagedLeaseClient {
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
        let events = match self.observed.present() {
            Ok(()) => vec![
                Ok(LlmEvent::TextDelta {
                    delta: MANAGED_ANSWER.to_string(),
                    meta: None,
                }),
                Ok(LlmEvent::UsageUpdate {
                    usage: meerkat_core::TurnUsage::host_declared(
                        Provider::OpenAI,
                        &request.model,
                        meerkat_core::Usage::default(),
                    ),
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::EndTurn),
                }),
            ],
            Err(message) => vec![Err(LlmError::InvalidRequest { message })],
        };
        Box::pin(futures::stream::iter(events))
    }

    fn provider(&self) -> Provider {
        Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

struct ManagedLeaseRuntime {
    observed: Arc<ManagedLease>,
}

#[async_trait]
impl ProviderRuntime for ManagedLeaseRuntime {
    fn provider_id(&self) -> Provider {
        Provider::OpenAI
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        self.observed.resolves.fetch_add(1, Ordering::SeqCst);
        if let Some(owner) = env.auth_lease_handle.clone() {
            *self.observed.lease.lock().unwrap() =
                Some((owner, binding.credential_identity().clone()));
        }
        Ok(ResolvedConnection {
            provider: Provider::OpenAI,
            backend: binding.backend(),
            backend_profile: Arc::clone(binding.backend_profile()),
            credential_identity: binding.credential_identity().clone(),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "managed-test-token".to_string(),
                meerkat_core::AuthMetadata::default(),
                Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                "prepared-controller-managed-test",
            )),
        })
    }

    fn build_client(
        &self,
        _connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Ok(Arc::new(ManagedLeaseClient {
            observed: Arc::clone(&self.observed),
        }))
    }

    fn build_text_client(
        &self,
        _target: ResolvedTextTarget,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Ok(Arc::new(ManagedLeaseClient {
            observed: Arc::clone(&self.observed),
        }))
    }
}
