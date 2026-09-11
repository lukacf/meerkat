//! The headline `brain_swap` proof: model A asks to become model B, and the
//! NEXT provider call is actually made on B.
//!
//! Everything else in this feature proves a fragment. The staging tests prove
//! the tool stages. The promotion tests prove a clean run boundary commits the
//! request. The realization saga proves the ordering of the rebinding steps.
//! None of them observe a provider call, so none of them can catch the failure
//! that matters to a user: the request is recorded, the log says `Realized`,
//! and the model answering the next turn is still A.
//!
//! This module closes that gap end to end, without credentials:
//!
//! * the `brain_swap` builtin is registered by the ORDINARY availability path
//!   — no manual tool registration, no injected dispatcher, no
//!   `default_llm_client`, no `llm_client_override`. Each of those makes the
//!   factory (correctly) advertise no reachable models, so a test that used
//!   one would be proving a tool production would never have offered;
//! * the provider layer is a fake `ProviderRuntime` registered in an otherwise
//!   empty `ProviderRuntimeRegistry`. It resolves non-secret managed binding
//!   identities to static test leases and returns a scripted client, so route
//!   resolution is real and the network is not;
//! * the cross-provider route uses Anthropic A (`claude-sonnet-4-6`) and
//!   OpenAI B (`gpt-5.5`) on distinct bindings that share one credential
//!   account identity;
//! * the turns are submitted through `accept_input_with_completion`, so the
//!   real runtime loop — including its pre-dequeue realization pass — is what
//!   moves the identity. Nothing here calls the realization method directly.
//!
//! Both durable persistence profiles run: WholeBlob
//! (`SqliteRuntimeStore::new_whole_blob` over a `JsonlStore`) and HeadCanonical
//! (`SqliteRuntimeStore::new_head_canonical` sharing the `SqliteSessionStore`
//! file). They commit session state through genuinely different store
//! authorities, and the handoff must cross both.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};

use async_trait::async_trait;
use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::image_generation::{SwitchTurnDuration, SwitchTurnOrigin, SwitchTurnRequestId};
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
};
use meerkat_core::session::model_routing_control::{
    ModelRoutingIntentRecordDisposition, SessionModelRoutingControlRecord,
};
use meerkat_core::{
    AuthBindingRef, BindingId, Config, Provider, RealmConfigSection, RealmId, Session, SessionId,
    SessionLlmIdentity, StopReason,
};
use meerkat_llm_core::provider_runtime::{
    ProviderAuthError, ProviderClientError, ProviderRuntime, ProviderRuntimeRegistry,
    ResolvedConnection, ResolvedTextTarget, ResolverEnvironment, StaticLease, ValidatedBinding,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::store::RuntimeStore;
use meerkat_runtime::{
    Input, LogicalRuntimeId, MeerkatMachine, PromptInput, SessionServiceRuntimeExt,
};
use meerkat_tools::builtin::brain_swap::BRAIN_SWAP_TOOL_NAME;

use crate::{
    AgentFactory, FactoryAgentBuilder, PersistenceBundle, PersistentSessionService, SessionStore,
};

/// The model the session starts on.
const MODEL_A: &str = "gpt-5.4";
/// The model the session asks to become, reachable on the SAME binding.
const MODEL_B: &str = "gpt-5.5";
const TEST_REALM: &str = "default";
const TEST_BINDING: &str = "default_openai";

/// The A and B routes a harness runs against.
///
/// Parameterizing this is what lets the SAME proof run same-provider and
/// cross-provider. A cross-provider swap is the interesting case for release:
/// it must re-resolve the provider, the credential binding, and the client, not
/// just swap a model string on an already-built route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RouteSpec {
    model_a: &'static str,
    provider_a: Provider,
    binding_a: &'static str,
    model_b: &'static str,
    provider_b: Provider,
    binding_b: &'static str,
}

impl RouteSpec {
    /// Every distinct provider this route can be served by, so the harness
    /// registers exactly the runtimes the route needs.
    fn providers(self) -> Vec<Provider> {
        if self.provider_a == self.provider_b {
            vec![self.provider_a]
        } else {
            vec![self.provider_a, self.provider_b]
        }
    }
}

const SAME_PROVIDER_ROUTE: RouteSpec = RouteSpec {
    model_a: MODEL_A,
    provider_a: Provider::OpenAI,
    binding_a: TEST_BINDING,
    model_b: MODEL_B,
    provider_b: Provider::OpenAI,
    binding_b: TEST_BINDING,
};

/// A=Anthropic, B=OpenAI. Both are catalog text models on separate configured
/// bindings, so realization must cross the provider seam.
const CROSS_PROVIDER_ROUTE: RouteSpec = RouteSpec {
    model_a: "claude-sonnet-4-6",
    provider_a: Provider::Anthropic,
    binding_a: "default_anthropic",
    model_b: MODEL_B,
    provider_b: Provider::OpenAI,
    binding_b: TEST_BINDING,
};

const AUTHORING_TEXT: &str = "staged the swap";
const NEXT_TURN_TEXT: &str = "answered after the swap";
const SECOND_NEXT_TURN_TEXT: &str = "answered the second queued turn";

// ---------------------------------------------------------------------------
// Scripted provider
// ---------------------------------------------------------------------------

/// Scripted client for the deferred (cross-run) brain swap.
///
/// It records the provider runtime and `request.model` for every call, which
/// distinguishes "the log says B" from "B actually served the turn".
#[derive(Clone, Debug, PartialEq, Eq)]
struct ProviderCall {
    provider: Provider,
    model: String,
}

struct DeferredBrainSwapClient {
    requested_calls: Arc<StdMutex<Vec<ProviderCall>>>,
    /// The provider this exact client was built for. Cross-provider routes
    /// build one client per provider, and usage must be attributed to the one
    /// that actually served the call.
    provider: Provider,
    route: RouteSpec,
}

impl DeferredBrainSwapClient {
    fn record(&self, model: &str) -> usize {
        let mut recorded = self
            .requested_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        recorded.push(ProviderCall {
            provider: self.provider,
            model: model.to_string(),
        });
        recorded.len() - 1
    }
}

fn usage_event(provider: Provider, model: &str) -> LlmEvent {
    LlmEvent::UsageUpdate {
        usage: meerkat_core::TurnUsage::host_declared(
            provider,
            model,
            meerkat_core::Usage::default(),
        ),
    }
}

fn done_event(stop_reason: StopReason) -> LlmEvent {
    LlmEvent::Done {
        outcome: LlmDoneOutcome::Success { stop_reason },
    }
}

#[async_trait]
impl LlmClient for DeferredBrainSwapClient {
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
        let call_index = self.record(&request.model);
        let events: Vec<Result<LlmEvent, LlmError>> = match call_index {
            // Call 0: the model asks, mid-run, to switch. The request is
            // staged; this run keeps answering on A.
            0 => vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: "toolu_brain_swap".to_string(),
                    name: BRAIN_SWAP_TOOL_NAME.to_string(),
                    args: serde_json::json!({ "target_model": self.route.model_b }),
                    meta: None,
                }),
                Ok(usage_event(self.provider, self.route.model_a)),
                Ok(done_event(StopReason::ToolUse)),
            ],
            // Call 1: the authoring turn reads its own tool result and
            // completes — still on A, which is the whole point of staging.
            1 => vec![
                Ok(LlmEvent::TextDelta {
                    delta: AUTHORING_TEXT.to_string(),
                    meta: None,
                }),
                Ok(usage_event(self.provider, self.route.model_a)),
                Ok(done_event(StopReason::EndTurn)),
            ],
            // Call 2: the next turn. Usage echoes whatever model the runtime
            // actually routed to, so a wrong route cannot hide behind a
            // hardcoded constant.
            2 => vec![
                Ok(LlmEvent::TextDelta {
                    delta: NEXT_TURN_TEXT.to_string(),
                    meta: None,
                }),
                Ok(usage_event(self.provider, &request.model)),
                Ok(done_event(StopReason::EndTurn)),
            ],
            // Call 3 is used by the concurrent-admission proof. Distinct text
            // makes a queue-order reversal observable through the two exact
            // completion handles.
            3 => vec![
                Ok(LlmEvent::TextDelta {
                    delta: SECOND_NEXT_TURN_TEXT.to_string(),
                    meta: None,
                }),
                Ok(usage_event(self.provider, &request.model)),
                Ok(done_event(StopReason::EndTurn)),
            ],
            // Anything past the script is a real finding, not noise to absorb.
            other => vec![Err(LlmError::InvalidRequest {
                message: format!(
                    "unscripted provider call #{other} on model {}",
                    request.model
                ),
            })],
        };
        Box::pin(futures::stream::iter(events))
    }

    fn provider(&self) -> Provider {
        self.provider
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Credential-free fake provider runtime.
///
/// `resolve_binding` performs the real validated-binding → resolved-connection
/// step using a static non-secret test lease, so the session resolves a
/// registry-backed provider identity — which is exactly what makes
/// `brain_swap` available at all. `build_client` hands back the scripted client
/// instead of an HTTP one.
struct DeferredBrainSwapProviderRuntime {
    provider: Provider,
    client: Arc<dyn LlmClient>,
    availability: Arc<ModelAvailability>,
    client_build_models: Arc<StdMutex<Vec<String>>>,
    resolved_auth_bindings: Arc<StdMutex<Vec<AuthBindingRef>>>,
}

#[derive(Default)]
struct ModelAvailability {
    unavailable: StdRwLock<BTreeSet<String>>,
}

impl ModelAvailability {
    fn set_available(&self, model: &str, available: bool) {
        let mut unavailable = self
            .unavailable
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if available {
            unavailable.remove(model);
        } else {
            unavailable.insert(model.to_string());
        }
    }

    fn is_available(&self, model: &str) -> bool {
        !self
            .unavailable
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(model)
    }
}

#[async_trait]
impl ProviderRuntime for DeferredBrainSwapProviderRuntime {
    fn provider_id(&self) -> Provider {
        self.provider
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        _env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        self.resolved_auth_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(binding.auth_binding_ref().clone());
        Ok(ResolvedConnection {
            provider: self.provider,
            backend: binding.backend(),
            backend_profile: Arc::clone(binding.backend_profile()),
            credential_identity: binding.credential_identity().clone(),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "unused-test-key".to_string(),
                meerkat_core::AuthMetadata::default(),
                None,
                format!("brain-swap-ab-test:{:?}", self.provider),
            )),
        })
    }

    fn build_client(
        &self,
        _connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        Ok(Arc::clone(&self.client))
    }

    fn build_text_client(
        &self,
        target: ResolvedTextTarget,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        let model = target.identity().model.clone();
        self.client_build_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(model.clone());
        if !self.availability.is_available(&model) {
            return Err(ProviderClientError::ClientInit(format!(
                "model '{model}' is unavailable in the fake provider runtime"
            )));
        }
        Ok(Arc::clone(&self.client))
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Which durable session-persistence authority the runtime store owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PersistenceProfile {
    WholeBlob,
    HeadCanonical,
}

fn ab_test_config(route: RouteSpec, availability: &ModelAvailability) -> Config {
    let mut config = Config::default();
    let account =
        meerkat_core::CredentialAccountId::parse("shared_test_account").expect("valid account");
    let mut section = RealmConfigSection::default();
    for provider in route.providers().into_iter().filter(|provider| {
        *provider == route.provider_b || availability.is_available(route.model_a)
    }) {
        let binding = if provider == route.provider_a {
            route.binding_a
        } else {
            route.binding_b
        };
        section.backend.insert(
            binding.to_string(),
            meerkat_core::BackendProfileConfig {
                provider: provider.as_str().to_string(),
                backend_kind: "copilot".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            },
        );
        section.auth.insert(
            binding.to_string(),
            meerkat_core::AuthProfileConfig {
                provider: provider.as_str().to_string(),
                auth_method: "github_copilot_oauth".to_string(),
                source: meerkat_core::CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            binding.to_string(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: binding.to_string(),
                auth_profile: binding.to_string(),
                credential_account: Some(account.clone()),
                default_model: None,
                policy: Default::default(),
                provider_default: false,
            },
        );
    }
    config.realm.insert(TEST_REALM.to_string(), section);
    config
}

fn expected_auth_binding_named(binding: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(TEST_REALM).expect("valid test realm"),
        binding: BindingId::parse(binding).expect("valid test binding"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

fn expected_auth_binding() -> AuthBindingRef {
    expected_auth_binding_named(TEST_BINDING)
}

#[test]
fn brain_swap_discovery_requires_configured_routes_without_foreign_auth() {
    let route = CROSS_PROVIDER_ROUTE;
    let availability = Arc::new(ModelAvailability::default());
    let config = ab_test_config(route, &availability);
    let current = SessionLlmIdentity {
        model: route.model_a.into(),
        provider: route.provider_a,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(expected_auth_binding_named(route.binding_a)),
    };
    let resolved_auth_bindings = Arc::new(StdMutex::new(Vec::new()));
    let client_build_models = Arc::new(StdMutex::new(Vec::new()));
    let provider_registry =
        ProviderRuntimeRegistry::empty().with_runtime(Arc::new(DeferredBrainSwapProviderRuntime {
            provider: route.provider_b,
            client: Arc::new(DeferredBrainSwapClient {
                requested_calls: Arc::new(StdMutex::new(Vec::new())),
                provider: route.provider_b,
                route,
            }),
            availability,
            client_build_models: Arc::clone(&client_build_models),
            resolved_auth_bindings: Arc::clone(&resolved_auth_bindings),
        }));
    let realm = RealmId::parse(TEST_REALM).unwrap();
    let models = |config: &Config, providers: &ProviderRuntimeRegistry| {
        super::brain_swap_available_models_for_resolved_identity(
            config,
            &config.model_registry(meerkat_models::canonical()).unwrap(),
            &current,
            providers,
            Some(&realm),
        )
    };
    assert!(
        models(&config, &provider_registry)
            .iter()
            .any(|m| m == route.model_b)
    );
    assert!(
        !models(&config, &ProviderRuntimeRegistry::empty())
            .iter()
            .any(|m| m == route.model_b)
    );
    assert!(
        !models(&config, &provider_registry)
            .iter()
            .any(|m| m == "unknown-brain-swap-model")
    );

    let mut unconfigured = config.clone();
    unconfigured
        .realm
        .get_mut(TEST_REALM)
        .unwrap()
        .binding
        .remove(route.binding_b);
    assert!(
        !models(&unconfigured, &provider_registry)
            .iter()
            .any(|m| m == route.model_b)
    );

    let mut wrong_account = config.clone();
    wrong_account
        .realm
        .get_mut(TEST_REALM)
        .unwrap()
        .binding
        .get_mut(route.binding_b)
        .unwrap()
        .credential_account =
        Some(meerkat_core::CredentialAccountId::parse("other_account").unwrap());
    assert!(
        !models(&wrong_account, &provider_registry)
            .iter()
            .any(|m| m == route.model_b)
    );

    let mut invalid_binding = config;
    invalid_binding
        .realm
        .get_mut(TEST_REALM)
        .unwrap()
        .auth
        .get_mut(route.binding_b)
        .unwrap()
        .auth_method = "unsupported-auth".into();
    assert!(
        !models(&invalid_binding, &provider_registry)
            .iter()
            .any(|m| m == route.model_b)
    );
    assert!(resolved_auth_bindings.lock().unwrap().is_empty());
    assert!(client_build_models.lock().unwrap().is_empty());
}

fn create_request_for(route: RouteSpec, fresh: bool) -> CreateSessionRequest {
    CreateSessionRequest {
        injected_context: Vec::new(),
        model: route.model_a.to_string(),
        prompt: meerkat_core::ContentInput::Text(String::new()),
        system_prompt: crate::SystemPromptOverride::Set("runtime-loop brain swap A->B".to_string()),
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions {
            provider: fresh.then_some(route.provider_a),
            realm_id: Some(RealmId::parse(TEST_REALM).expect("valid test realm")),
            auth_binding: fresh.then(|| expected_auth_binding_named(route.binding_a)),
            override_image_generation: meerkat_core::ToolCategoryOverride::Disable,
            ..Default::default()
        }),
        labels: None,
    }
}

struct AbHarness {
    service: Arc<PersistentSessionService<FactoryAgentBuilder>>,
    adapter: Arc<MeerkatMachine>,
    runtime_store: Arc<dyn RuntimeStore>,
    session_id: SessionId,
    runtime_id: LogicalRuntimeId,
    environment: Arc<AbEnvironment>,
}

struct AbEnvironment {
    profile: PersistenceProfile,
    route: RouteSpec,
    temp: tempfile::TempDir,
    requested_calls: Arc<StdMutex<Vec<ProviderCall>>>,
    availability: Arc<ModelAvailability>,
    client_build_models: Arc<StdMutex<Vec<String>>>,
    resolved_auth_bindings: Arc<StdMutex<Vec<AuthBindingRef>>>,
}

impl AbHarness {
    fn requested_models(&self) -> Vec<String> {
        self.environment
            .requested_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|call| call.model.clone())
            .collect()
    }

    fn requested_calls(&self) -> Vec<ProviderCall> {
        self.environment
            .requested_calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_available(&self, model: &str, available: bool) {
        self.environment
            .availability
            .set_available(model, available);
    }

    fn clear_client_build_models(&self) {
        self.environment
            .client_build_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn client_build_models(&self) -> Vec<String> {
        self.environment
            .client_build_models
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn resolved_auth_bindings(&self) -> Vec<AuthBindingRef> {
        self.environment
            .resolved_auth_bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// How many committed realizations the durable log actually carries.
    ///
    /// This replaces an earlier decorator that counted calls into a wrapped
    /// reconfigure service. The harness now builds through the production
    /// composition, which installs the canonical host — so there is no seam to
    /// decorate, and there should not be: a second host would be exactly the
    /// shadow this feature must not have. The durable count is the stronger
    /// fact anyway, because a second rotation would have to leave a second
    /// committed terminal behind to be real.
    async fn realized_terminal_count(&self) -> usize {
        self.durable_routing_records()
            .await
            .iter()
            .filter(|record| record.disposition() == ModelRoutingIntentRecordDisposition::Realized)
            .count()
    }

    async fn accept_prompt(&self, prompt: &str) -> (InputId, meerkat_runtime::CompletionHandle) {
        let (accepted, completion) = self
            .adapter
            .accept_input_with_completion(
                &self.session_id,
                Input::Prompt(PromptInput::new(prompt, None)),
            )
            .await
            .expect("runtime must accept the prompt input");
        let completion = completion.expect("an accepted prompt must yield a completion handle");
        let input_id = match accepted {
            meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } => input_id,
            other => panic!("a fresh prompt must be accepted, got {other:?}"),
        };
        (input_id, completion)
    }

    async fn wait_completion(completion: meerkat_runtime::CompletionHandle) -> CompletionOutcome {
        tokio::time::timeout(std::time::Duration::from_secs(5), completion.wait())
            .await
            .expect("the turn must settle within the deterministic budget")
            .expect("the completion waiter must resolve")
    }

    async fn run_prompt(&self, prompt: &str) -> CompletionOutcome {
        let (_input_id, completion) = self.accept_prompt(prompt).await;
        Self::wait_completion(completion).await
    }

    async fn authoritative_session(&self) -> Session {
        self.service
            .load_authoritative_session(&self.session_id)
            .await
            .expect("load the authoritative session")
            .expect("a runtime-backed session must be durable")
    }

    /// The durable model-routing control log, read back off the authoritative
    /// session rather than off any live in-memory copy.
    async fn durable_routing_records(&self) -> Vec<SessionModelRoutingControlRecord> {
        self.authoritative_session()
            .await
            .model_routing_control()
            .records()
            .to_vec()
    }

    async fn durable_identity(&self) -> SessionLlmIdentity {
        self.authoritative_session()
            .await
            .session_metadata()
            .expect("a runtime-backed session must retain canonical metadata")
            .llm_identity()
    }

    async fn live_identity(&self) -> SessionLlmIdentity {
        self.service
            .live_session_llm_identity(&self.session_id)
            .await
            .expect("read the live identity watch")
    }

    /// Whether `run` actually committed a run-boundary receipt — the exact
    /// evidence realization requires before it will move routing.
    async fn origin_run_committed_a_boundary(&self, run: &RunId) -> bool {
        !self
            .runtime_store
            .load_committed_boundary_receipts(&self.runtime_id, run)
            .await
            .expect("read committed boundary receipts")
            .is_empty()
    }

    async fn cold_restart(self) -> Self {
        let environment = Arc::clone(&self.environment);
        let session_id = self.session_id.clone();
        self.adapter
            .unregister_session(&session_id)
            .await
            .expect("tear down the original runtime attachment");
        drop(self);
        attach_harness(environment, Some(session_id)).await
    }
}

async fn build_harness(profile: PersistenceProfile) -> AbHarness {
    build_harness_for(profile, SAME_PROVIDER_ROUTE).await
}

async fn build_harness_for(profile: PersistenceProfile, route: RouteSpec) -> AbHarness {
    let environment = Arc::new(AbEnvironment {
        profile,
        route,
        temp: tempfile::tempdir().expect("brain-swap A/B tempdir"),
        requested_calls: Arc::new(StdMutex::new(Vec::new())),
        availability: Arc::new(ModelAvailability::default()),
        client_build_models: Arc::new(StdMutex::new(Vec::new())),
        resolved_auth_bindings: Arc::new(StdMutex::new(Vec::new())),
    });
    attach_harness(environment, None).await
}

async fn attach_harness(
    environment: Arc<AbEnvironment>,
    resume_session_id: Option<SessionId>,
) -> AbHarness {
    let route = environment.route;
    let mut factory = AgentFactory::new(environment.temp.path().join("factory-sessions"))
        .without_provider_auth_persistence()
        .builtins(true);
    // One runtime per provider the route can be served by. The call script is
    // shared through `requested_calls`, so a cross-provider swap still reads
    // as one ordered sequence of provider calls.
    let mut registry = ProviderRuntimeRegistry::empty();
    for provider in route.providers().into_iter().filter(|provider| {
        *provider == route.provider_b || environment.availability.is_available(route.model_a)
    }) {
        let scripted: Arc<dyn LlmClient> = Arc::new(DeferredBrainSwapClient {
            requested_calls: Arc::clone(&environment.requested_calls),
            provider,
            route,
        });
        registry = registry.with_runtime(Arc::new(DeferredBrainSwapProviderRuntime {
            provider,
            client: scripted,
            availability: Arc::clone(&environment.availability),
            client_build_models: Arc::clone(&environment.client_build_models),
            resolved_auth_bindings: Arc::clone(&environment.resolved_auth_bindings),
        }));
    }
    factory.provider_registry = Arc::new(registry);

    let config = ab_test_config(route, environment.availability.as_ref());
    if environment.availability.is_available(route.model_a) {
        meerkat_core::resolve_explicit_auth_binding_target(
            &config,
            &expected_auth_binding_named(route.binding_a),
        )
        .expect("A's configured binding must resolve in the A/B harness");
    }
    let builder = FactoryAgentBuilder::new(factory, config);
    assert!(
        builder.default_llm_client.is_none(),
        "the A->B proof must resolve every client through the provider registry"
    );

    let sqlite_path = environment.temp.path().join("sessions.sqlite3");
    let (session_store, runtime_store): (Arc<dyn SessionStore>, Arc<dyn RuntimeStore>) =
        match environment.profile {
            PersistenceProfile::WholeBlob => (
                Arc::new(meerkat_store::JsonlStore::new(
                    environment.temp.path().join("jsonl"),
                )),
                Arc::new(
                    meerkat_runtime::store::SqliteRuntimeStore::new_whole_blob(
                        environment.temp.path().join("runtime.sqlite3"),
                    )
                    .expect("open the whole-blob runtime store"),
                ),
            ),
            PersistenceProfile::HeadCanonical => (
                Arc::new(
                    meerkat_store::SqliteSessionStore::open(sqlite_path.clone())
                        .expect("open the sqlite session store"),
                ),
                Arc::new(
                    meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(sqlite_path)
                        .expect("open the head-canonical runtime store"),
                ),
            ),
        };
    let persistence = PersistenceBundle::new(
        Arc::clone(&session_store),
        Arc::clone(&runtime_store),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    );

    // Built through the PRODUCTION composition, not a hand-wired blueprint.
    // The canonical reconfigure host is installed by the same code path every
    // product surface uses, so this test proves the shipped wiring realizes a
    // handoff — a harness that installed its own host would prove only that
    // the harness works.
    let (service, adapter) =
        crate::surface::build_runtime_backed_service_with_default_reconfigure_host(
            builder,
            4,
            persistence,
            environment.temp.path().join("config_state.json"),
        );

    let fresh = resume_session_id.is_none();
    let session = match resume_session_id {
        Some(session_id) => service
            .load_authoritative_session(&session_id)
            .await
            .expect("load the retained authoritative session after restart")
            .expect("the retained session must remain durable"),
        None => Session::new(),
    };
    let session_id = session.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let materialized = Box::pin(crate::surface::materialize_session(
        &service,
        &adapter,
        session,
        create_request_for(route, fresh),
        {
            let service = Arc::clone(&service);
            let adapter = Arc::clone(&adapter);
            move |session_id| {
                crate::surface::default_persistent_executor(service, adapter, session_id)
            }
        },
    ))
    .await
    .expect("materialize the runtime-backed session with a real executor");
    assert_eq!(materialized.session_id, session_id);

    AbHarness {
        service,
        adapter,
        runtime_store,
        session_id,
        runtime_id,
        environment,
    }
}

struct CommittedHandoffProof {
    request_id: SwitchTurnRequestId,
    origin_run: RunId,
    auth_binding: Option<AuthBindingRef>,
    route: RouteSpec,
}

async fn commit_brain_swap_request(harness: &AbHarness) -> CommittedHandoffProof {
    let route = harness.environment.route;
    let auth_binding = harness.live_identity().await.auth_binding;

    let authoring = harness.run_prompt("please switch after this turn").await;
    assert!(
        matches!(authoring, CompletionOutcome::Completed(ref run) if run.text == AUTHORING_TEXT),
        "the authoring turn must complete on A: {authoring:?}"
    );
    assert_eq!(
        harness.requested_models(),
        vec![route.model_a.to_string(), route.model_a.to_string()]
    );
    assert_eq!(harness.live_identity().await.model, route.model_a);
    assert_eq!(harness.durable_identity().await.model, route.model_a);

    let records = harness.durable_routing_records().await;
    assert_eq!(records.len(), 1);
    let (request_id, origin_run) = match &records[0] {
        SessionModelRoutingControlRecord::ModelRoutingIntentRequested {
            request_id,
            originating_run_id,
            intent,
        } => {
            assert_eq!(intent.target_model.as_str(), route.model_b);
            (*request_id, originating_run_id.clone())
        }
        other => panic!("expected one committed Requested record, got {other:?}"),
    };
    assert!(
        harness.origin_run_committed_a_boundary(&origin_run).await,
        "the authoring run must have an exact committed boundary receipt"
    );
    CommittedHandoffProof {
        request_id,
        origin_run,
        auth_binding,
        route,
    }
}

// ---------------------------------------------------------------------------
// The proof
// ---------------------------------------------------------------------------

async fn brain_swap_moves_the_next_provider_call_from_a_to_b(profile: PersistenceProfile) {
    let harness = build_harness(profile).await;
    let binding_before = harness.live_identity().await.auth_binding;
    assert_eq!(
        binding_before.as_ref(),
        Some(&expected_auth_binding()),
        "both models must be reachable through one configured binding"
    );

    // ---- Authoring run: the model requests the switch and keeps answering on
    // ---- A for the rest of that run.
    let authoring = harness.run_prompt("please switch after this turn").await;
    assert!(
        matches!(authoring, CompletionOutcome::Completed(ref run) if run.text == AUTHORING_TEXT),
        "the authoring turn must complete on the original model: {authoring:?}"
    );
    assert_eq!(
        harness.requested_models(),
        vec![MODEL_A.to_string(), MODEL_A.to_string()],
        "every provider call in the requesting run must stay on model A"
    );
    assert_eq!(
        harness.live_identity().await.model,
        MODEL_A,
        "staging must not move live routing"
    );
    assert_eq!(
        harness.durable_identity().await.model,
        MODEL_A,
        "staging must not move durable routing"
    );

    let records = harness.durable_routing_records().await;
    assert_eq!(
        records.len(),
        1,
        "exactly one durable handoff record must exist after the authoring run: {records:?}"
    );
    let (request_id, origin_run) = match &records[0] {
        SessionModelRoutingControlRecord::ModelRoutingIntentRequested {
            request_id,
            originating_run_id,
            intent,
        } => {
            assert_eq!(intent.target_model.as_str(), MODEL_B);
            assert!(matches!(intent.duration, SwitchTurnDuration::UntilChanged));
            assert!(matches!(intent.origin, SwitchTurnOrigin::Model { .. }));
            (*request_id, originating_run_id.clone())
        }
        other => panic!("the authoring run must commit a Requested record, got {other:?}"),
    };
    assert_ne!(
        request_id,
        SwitchTurnRequestId::new(uuid::Uuid::nil()),
        "the tool must mint a real request identity"
    );
    assert_eq!(
        records[0].disposition(),
        ModelRoutingIntentRecordDisposition::Requested,
        "the request must not be terminal before the next input is admitted"
    );
    assert!(
        records[0].disposition().is_awaiting_decision(),
        "a staged-then-committed request is the one waiting disposition"
    );
    assert!(
        harness.origin_run_committed_a_boundary(&origin_run).await,
        "the durable request must be bound to a run that really committed a boundary"
    );

    let awaiting = harness
        .adapter
        .committed_model_routing_handoffs_awaiting_decision(&harness.session_id)
        .await
        .expect("read committed handoffs awaiting decision");
    assert_eq!(
        awaiting.len(),
        1,
        "the committed request must be visible to the loop as pending: {awaiting:?}"
    );
    assert_eq!(awaiting[0].request_id, request_id);
    assert_eq!(awaiting[0].originating_run_id, origin_run);

    // ---- Next input: nothing below calls realization. The runtime loop's
    // ---- pre-dequeue pass must do it before this input reaches a provider.
    let next = harness.run_prompt("continue").await;
    assert!(
        matches!(next, CompletionOutcome::Completed(ref run) if run.text == NEXT_TURN_TEXT),
        "the next turn must complete: {next:?}"
    );
    assert_eq!(
        harness.requested_models(),
        vec![
            MODEL_A.to_string(),
            MODEL_A.to_string(),
            MODEL_B.to_string(),
        ],
        "the first and only provider call after the handoff must be on model B"
    );

    assert_eq!(
        harness.live_identity().await.model,
        MODEL_B,
        "live routing must be model B"
    );
    let durable_identity = harness.durable_identity().await;
    assert_eq!(durable_identity.model, MODEL_B, "durable routing must be B");
    assert_eq!(durable_identity.provider, Provider::OpenAI);
    assert_eq!(
        durable_identity.auth_binding, binding_before,
        "a model-only switch must not rebind credentials or accounts"
    );

    let records = harness.durable_routing_records().await;
    assert_eq!(
        records.len(),
        2,
        "the append-only log must hold exactly the request and its one terminal: {records:?}"
    );
    assert_eq!(
        records[0].disposition(),
        ModelRoutingIntentRecordDisposition::Requested,
        "the original request record must survive verbatim"
    );
    let terminal = records
        .iter()
        .rfind(|record| *record.request_id() == request_id)
        .expect("the request must retain a durable record");
    match terminal {
        SessionModelRoutingControlRecord::ModelRoutingIntentRealized {
            originating_run_id,
            applied_identity,
            ..
        } => {
            assert_eq!(
                originating_run_id, &origin_run,
                "the terminal record must stay bound to the originating run"
            );
            assert_eq!(applied_identity.model, MODEL_B);
            assert_eq!(applied_identity.auth_binding, binding_before);
        }
        other => panic!("the realized handoff must record a Realized terminal, got {other:?}"),
    }
    assert_eq!(
        terminal.disposition(),
        ModelRoutingIntentRecordDisposition::Realized
    );
    assert!(
        terminal.disposition().is_terminal(),
        "the request must be terminal after realization"
    );

    let awaiting = harness
        .adapter
        .committed_model_routing_handoffs_awaiting_decision(&harness.session_id)
        .await
        .expect("read committed handoffs awaiting decision");
    assert!(
        awaiting.is_empty(),
        "no handoff may still await a decision after realization: {awaiting:?}"
    );
}

#[tokio::test]
async fn whole_blob_brain_swap_moves_the_next_provider_call_from_a_to_b() {
    brain_swap_moves_the_next_provider_call_from_a_to_b(PersistenceProfile::WholeBlob).await;
}

#[tokio::test]
async fn head_canonical_brain_swap_moves_the_next_provider_call_from_a_to_b() {
    brain_swap_moves_the_next_provider_call_from_a_to_b(PersistenceProfile::HeadCanonical).await;
}

fn assert_exact_realized_terminal(
    records: &[SessionModelRoutingControlRecord],
    proof: &CommittedHandoffProof,
) {
    assert_eq!(
        records.len(),
        2,
        "one Requested and one terminal must be durable: {records:?}"
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| record.request_id() == &proof.request_id)
            .count(),
        2,
        "the request must not be duplicated or rotated twice"
    );
    match &records[1] {
        SessionModelRoutingControlRecord::ModelRoutingIntentRealized {
            request_id,
            originating_run_id,
            applied_identity,
            ..
        } => {
            assert_eq!(request_id, &proof.request_id);
            assert_eq!(originating_run_id, &proof.origin_run);
            assert_eq!(applied_identity.model, proof.route.model_b);
            assert_eq!(applied_identity.provider, proof.route.provider_b);
            assert_eq!(
                applied_identity.auth_binding,
                Some(expected_auth_binding_named(proof.route.binding_b))
            );
        }
        other => panic!("expected a durable Realized terminal, got {other:?}"),
    }
}

/// The cross-provider proof: A=Anthropic, B=OpenAI, on separate configured
/// bindings.
///
/// A same-provider swap can pass while realization only rewrites a model
/// string on an already-built route. This one cannot: realizing B has to
/// re-resolve the provider, pick the OTHER credential binding, and build a
/// different client. The durable `Realized` record must say so, and the next
/// provider call must actually land on OpenAI.
///
/// No live credentials: both providers are scripted runtimes with non-secret
/// managed binding identities and static test leases, and there is no
/// provider-specific branch in production code.
async fn brain_swap_crosses_the_provider_seam(profile: PersistenceProfile) {
    let route = CROSS_PROVIDER_ROUTE;
    let harness = build_harness_for(profile, route).await;
    let config = ab_test_config(route, &harness.environment.availability);
    assert!(!config.model_fallback.is_enabled());
    assert!(!config.model_fallback.policy.cross_provider);
    assert_eq!(harness.client_build_models(), vec![route.model_a]);
    assert_eq!(
        harness.resolved_auth_bindings(),
        vec![expected_auth_binding_named(route.binding_a)],
        "advertising explicit brain-swap targets must not resolve foreign credentials"
    );

    // The session really does start on Anthropic's route.
    let before = harness.live_identity().await;
    assert_eq!(before.model, route.model_a);
    assert_eq!(before.provider, route.provider_a);
    assert_eq!(
        before.auth_binding,
        Some(expected_auth_binding_named(route.binding_a)),
        "the authoring route must use A's configured non-secret binding identity"
    );

    let proof = commit_brain_swap_request(&harness).await;
    assert_eq!(harness.client_build_models(), vec![route.model_a]);
    assert_eq!(
        harness.resolved_auth_bindings(),
        vec![expected_auth_binding_named(route.binding_a)],
        "staging must not resolve the target before explicit runtime realization"
    );
    let next = harness
        .run_prompt("continue after the cross-provider swap")
        .await;
    assert!(
        matches!(next, CompletionOutcome::Completed(ref run) if run.text == NEXT_TURN_TEXT),
        "the next turn must complete after pre-dequeue realizes B: {next:?}"
    );
    assert_eq!(
        harness.requested_models(),
        vec![
            route.model_a.to_string(),
            route.model_a.to_string(),
            route.model_b.to_string(),
        ],
        "both authoring calls stay on A and the next call is B"
    );
    assert_eq!(
        harness.requested_calls(),
        vec![
            ProviderCall {
                provider: route.provider_a,
                model: route.model_a.to_string(),
            },
            ProviderCall {
                provider: route.provider_a,
                model: route.model_a.to_string(),
            },
            ProviderCall {
                provider: route.provider_b,
                model: route.model_b.to_string(),
            },
        ],
        "the first post-handoff call must be served by B's provider runtime"
    );

    let after = harness.durable_identity().await;
    assert_eq!(after.model, route.model_b);
    assert_eq!(
        after.provider, route.provider_b,
        "durable routing must have crossed the provider seam"
    );
    // The one thing a cross-provider swap must never do is keep serving on the
    // credential it left behind.
    assert_ne!(
        after.auth_binding, proof.auth_binding,
        "a cross-provider swap must not keep A's binding"
    );
    assert_eq!(
        after.auth_binding,
        Some(expected_auth_binding_named(route.binding_b)),
        "the realized identity must durably name B's non-secret binding identity"
    );
    assert_exact_realized_terminal(&harness.durable_routing_records().await, &proof);

    // Cold restart with A's runtime gone. The durable Realized identity must
    // stand on its own: nothing may fall back to the provider the session was
    // created on.
    harness.set_available(route.model_a, false);
    harness.clear_client_build_models();
    let harness = harness.cold_restart().await;

    assert_eq!(
        harness.durable_identity().await.model,
        route.model_b,
        "a committed cross-provider realization must survive an ordinary restart"
    );
    assert!(
        !harness
            .client_build_models()
            .iter()
            .any(|model| model == route.model_a),
        "cold materialization must not try to rebuild the abandoned A route: {:?}",
        harness.client_build_models()
    );

    let resumed = harness
        .run_prompt("continue after the cross-provider restart")
        .await;
    assert!(
        matches!(resumed, CompletionOutcome::Completed(ref run) if run.text == SECOND_NEXT_TURN_TEXT),
        "the restarted session must answer on B with A unavailable: {resumed:?}"
    );
    assert_eq!(
        harness.requested_models(),
        vec![
            route.model_a.to_string(),
            route.model_a.to_string(),
            route.model_b.to_string(),
            route.model_b.to_string(),
        ],
        "the restarted turn must also be served by B"
    );
    assert_eq!(
        harness.requested_calls().last(),
        Some(&ProviderCall {
            provider: route.provider_b,
            model: route.model_b.to_string(),
        }),
        "the restarted call must also be served by B's provider runtime"
    );
    assert_eq!(
        harness.live_identity().await.provider,
        route.provider_b,
        "the restarted live session must be bound to B's provider"
    );
}

#[tokio::test]
async fn whole_blob_brain_swap_crosses_the_provider_seam() {
    brain_swap_crosses_the_provider_seam(PersistenceProfile::WholeBlob).await;
}

#[tokio::test]
async fn head_canonical_brain_swap_crosses_the_provider_seam() {
    brain_swap_crosses_the_provider_seam(PersistenceProfile::HeadCanonical).await;
}

async fn cold_restart_realizes_with_a_unavailable(profile: PersistenceProfile) {
    let harness = build_harness(profile).await;
    let proof = commit_brain_swap_request(&harness).await;
    harness.set_available(MODEL_A, false);
    harness.set_available(MODEL_B, true);
    harness.clear_client_build_models();

    let harness = harness.cold_restart().await;
    assert!(
        harness
            .client_build_models()
            .starts_with(&[MODEL_A.to_string(), MODEL_B.to_string()]),
        "cold materialization must observe A unavailable, then bootstrap from exact Requested B: {:?}",
        harness.client_build_models()
    );
    assert_eq!(
        harness.requested_models(),
        vec![MODEL_A.to_string(), MODEL_A.to_string()],
        "cold materialization may construct B but must not call a provider"
    );
    assert_eq!(
        harness.durable_identity().await.model,
        MODEL_A,
        "bootstrap must not mark the request realized or rewrite durable identity"
    );
    assert_eq!(harness.realized_terminal_count().await, 0);
    let waiting = harness.durable_routing_records().await;
    assert_eq!(waiting.len(), 1);
    assert_eq!(
        waiting[0].disposition(),
        ModelRoutingIntentRecordDisposition::Requested
    );

    let next = harness.run_prompt("continue after cold restart").await;
    assert!(
        matches!(next, CompletionOutcome::Completed(ref run) if run.text == NEXT_TURN_TEXT),
        "the retained next input must complete after pre-dequeue realizes B: {next:?}"
    );
    assert_eq!(
        harness.requested_models(),
        vec![
            MODEL_A.to_string(),
            MODEL_A.to_string(),
            MODEL_B.to_string(),
        ]
    );
    assert_eq!(
        harness.realized_terminal_count().await,
        1,
        "the cold successor must apply B exactly once"
    );
    assert_eq!(harness.live_identity().await.model, MODEL_B);
    assert_exact_realized_terminal(&harness.durable_routing_records().await, &proof);
}

#[tokio::test]
async fn whole_blob_cold_restart_bootstraps_from_requested_b_when_a_is_unavailable() {
    cold_restart_realizes_with_a_unavailable(PersistenceProfile::WholeBlob).await;
}

#[tokio::test]
async fn head_canonical_cold_restart_bootstraps_from_requested_b_when_a_is_unavailable() {
    cold_restart_realizes_with_a_unavailable(PersistenceProfile::HeadCanonical).await;
}

async fn unavailable_b_holds_the_real_next_input(profile: PersistenceProfile) {
    let harness = build_harness(profile).await;
    let proof = commit_brain_swap_request(&harness).await;
    harness.set_available(MODEL_B, false);
    harness.clear_client_build_models();

    let (input_id, completion) = harness.accept_prompt("must remain unattempted").await;
    let outcome = AbHarness::wait_completion(completion).await;
    assert!(
        !matches!(outcome, CompletionOutcome::Completed(_)),
        "an unavailable target must not report a successful completion: {outcome:?}"
    );
    assert_eq!(
        harness.requested_models(),
        vec![MODEL_A.to_string(), MODEL_A.to_string()],
        "the held input must make zero provider calls"
    );
    assert!(
        harness
            .client_build_models()
            .iter()
            .any(|model| model == MODEL_B),
        "the real pre-dequeue path must test B through provider-runtime availability"
    );
    let stored = harness
        .runtime_store
        .load_input_state(&harness.runtime_id, &input_id)
        .await
        .expect("read the held input row")
        .expect("the held input must retain durable state");
    assert_eq!(
        stored.seed.attempt_count, 0,
        "pre-dequeue must hold before the input gains an execution attempt"
    );

    let records = harness.durable_routing_records().await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].request_id(), &proof.request_id);
    assert_eq!(
        records[0].disposition(),
        ModelRoutingIntentRecordDisposition::Requested,
        "a retriable target-unavailable hold must leave Requested waiting"
    );
    assert_eq!(
        harness.realized_terminal_count().await,
        0,
        "target preflight must fail before any realization is committed"
    );
}

#[tokio::test]
async fn whole_blob_unavailable_b_holds_the_real_next_input() {
    unavailable_b_holds_the_real_next_input(PersistenceProfile::WholeBlob).await;
}

#[tokio::test]
async fn head_canonical_unavailable_b_holds_the_real_next_input() {
    unavailable_b_holds_the_real_next_input(PersistenceProfile::HeadCanonical).await;
}

async fn concurrent_next_admissions_realize_once_in_order(profile: PersistenceProfile) {
    let harness = build_harness(profile).await;
    let proof = commit_brain_swap_request(&harness).await;

    let turn_boundary = harness
        .service
        .acquire_runtime_turn_finalization_guard(&harness.session_id)
        .await;
    let (first_id, first_completion) = harness.accept_prompt("queued first").await;
    let (second_id, second_completion) = harness.accept_prompt("queued second").await;
    assert_ne!(first_id, second_id);
    assert_eq!(
        harness
            .adapter
            .committed_model_routing_handoffs_awaiting_decision(&harness.session_id)
            .await
            .expect("the request remains visible while admission is fenced")
            .len(),
        1,
        "both inputs must be admitted while the one committed request is pending"
    );
    drop(turn_boundary);

    let first = AbHarness::wait_completion(first_completion).await;
    let second = AbHarness::wait_completion(second_completion).await;
    assert!(
        matches!(first, CompletionOutcome::Completed(ref run) if run.text == NEXT_TURN_TEXT),
        "the first admitted input must receive the first B result: {first:?}"
    );
    assert!(
        matches!(second, CompletionOutcome::Completed(ref run) if run.text == SECOND_NEXT_TURN_TEXT),
        "the second admitted input must receive the second B result: {second:?}"
    );
    assert_eq!(
        harness.requested_models(),
        vec![
            MODEL_A.to_string(),
            MODEL_A.to_string(),
            MODEL_B.to_string(),
            MODEL_B.to_string(),
        ],
        "both queued calls must run on B in admission order"
    );
    assert_eq!(
        harness.realized_terminal_count().await,
        1,
        "one pending request must commit exactly one realization"
    );

    let first_state = harness
        .runtime_store
        .load_input_state(&harness.runtime_id, &first_id)
        .await
        .expect("read first input state")
        .expect("first input state remains durable");
    let second_state = harness
        .runtime_store
        .load_input_state(&harness.runtime_id, &second_id)
        .await
        .expect("read second input state")
        .expect("second input state remains durable");
    assert_eq!(first_state.seed.attempt_count, 1);
    assert_eq!(second_state.seed.attempt_count, 1);
    assert!(
        first_state.seed.admission_sequence < second_state.seed.admission_sequence,
        "generated admission sequence must preserve input order"
    );

    let user_inputs = harness
        .authoritative_session()
        .await
        .messages()
        .iter()
        .filter_map(|message| match message {
            meerkat_core::Message::User(user) => Some(user.text_content()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        user_inputs.ends_with(&["queued first".to_string(), "queued second".to_string()]),
        "durable transcript order must match admission order: {user_inputs:?}"
    );
    assert_exact_realized_terminal(&harness.durable_routing_records().await, &proof);
}

#[tokio::test]
async fn whole_blob_concurrent_next_admissions_realize_once_in_order() {
    concurrent_next_admissions_realize_once_in_order(PersistenceProfile::WholeBlob).await;
}

#[tokio::test]
async fn head_canonical_concurrent_next_admissions_realize_once_in_order() {
    concurrent_next_admissions_realize_once_in_order(PersistenceProfile::HeadCanonical).await;
}
