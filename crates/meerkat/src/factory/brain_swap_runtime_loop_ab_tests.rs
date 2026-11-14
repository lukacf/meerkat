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
/// How long a scripted turn may take to settle before a row calls it hung.
/// The scripted responses answer at once, so the bound only turns a hang into
/// a failure; it leaves room for a loaded lane, where a 5 s bound timed out in
/// plain `cargo test` runs of the whole library.
const TURN_SETTLE_GUARD: std::time::Duration = std::time::Duration::from_mins(1);
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

/// A=OpenAI `gpt-5.5`, a model that lowers operator-instruction entries
/// anywhere, and B=Anthropic `claude-sonnet-4-6`, a model that lowers them
/// only inside the leading System prefix, on bindings that share one
/// credential account.
const TO_LEADING_PREFIX_ONLY_ROUTE: RouteSpec = RouteSpec {
    model_a: MODEL_B,
    provider_a: Provider::OpenAI,
    binding_a: TEST_BINDING,
    model_b: "claude-sonnet-4-6",
    provider_b: Provider::Anthropic,
    binding_b: "default_anthropic",
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
    /// The route this client reports for the provider-neutral reasoning
    /// effort option; `None` (the default) reports no route.
    reasoning_route: ScriptedReasoningRoute,
}

/// A route observation a row scripts for every client of its environment.
type ScriptedReasoningRoute = Arc<StdMutex<Option<meerkat_core::ReasoningRouteObservation>>>;

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
        outcome: LlmDoneOutcome::success(stop_reason),
    }
}

#[async_trait]
impl LlmClient for DeferredBrainSwapClient {
    fn reasoning_route(&self) -> Option<meerkat_core::ReasoningRouteObservation> {
        self.reasoning_route
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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
                reasoning_route: ScriptedReasoningRoute::default(),
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
    reasoning_route: ScriptedReasoningRoute,
    /// Whether the configuration enables model fallback, with the route's
    /// model B as the one chain target.
    fallback_to_b: bool,
}

impl AbHarness {
    /// The route every client of this environment reports from now on.
    fn set_reasoning_route(&self, route: Option<meerkat_core::ReasoningRouteObservation>) {
        *self
            .environment
            .reasoning_route
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = route;
    }

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
        tokio::time::timeout(TURN_SETTLE_GUARD, completion.wait())
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
        let (environment, session_id) = self.detach().await;
        attach_harness(environment, Some(session_id)).await
    }

    /// Tear the runtime attachment down, as a restart does before it
    /// reattaches the same session, and hand back what reattaching needs.
    async fn detach(self) -> (Arc<AbEnvironment>, SessionId) {
        let environment = Arc::clone(&self.environment);
        let session_id = self.session_id.clone();
        // A restart reattaches the same session, so it must observe terminal
        // teardown of the exact original registration first. The plain
        // unregister bounds its wait by a caller grace and answers the
        // retryable `UnregisterInProgress` while a slow teardown (under load)
        // is still running.
        let registration = self
            .adapter
            .current_session_registration_witness(&session_id)
            .await
            .expect("the original attachment has a current registration");
        let torn_down = self
            .adapter
            .unregister_session_registration_until_terminal_if_current(&registration)
            .await
            .expect("tear down the original runtime attachment");
        assert!(torn_down, "the original registration was no longer current");
        drop(self);
        (environment, session_id)
    }
}

async fn build_harness(profile: PersistenceProfile) -> AbHarness {
    build_harness_for(profile, SAME_PROVIDER_ROUTE).await
}

async fn build_harness_for(profile: PersistenceProfile, route: RouteSpec) -> AbHarness {
    attach_harness(new_environment(profile, route), None).await
}

/// A harness whose session had `seed`'s rows before the harness built it.
async fn build_harness_seeded(
    profile: PersistenceProfile,
    route: RouteSpec,
    seed: Session,
) -> AbHarness {
    try_attach_harness(new_environment(profile, route), HarnessSession::Fresh(seed))
        .await
        .expect("materialize the runtime-backed session with a real executor")
}

fn new_environment(profile: PersistenceProfile, route: RouteSpec) -> Arc<AbEnvironment> {
    new_environment_with(profile, route, false)
}

/// [`new_environment`]; with `fallback_to_b` the configuration enables model
/// fallback and names the route's model B as the one chain target.
fn new_environment_with(
    profile: PersistenceProfile,
    route: RouteSpec,
    fallback_to_b: bool,
) -> Arc<AbEnvironment> {
    Arc::new(AbEnvironment {
        profile,
        route,
        temp: tempfile::tempdir().expect("brain-swap A/B tempdir"),
        requested_calls: Arc::new(StdMutex::new(Vec::new())),
        availability: Arc::new(ModelAvailability::default()),
        client_build_models: Arc::new(StdMutex::new(Vec::new())),
        resolved_auth_bindings: Arc::new(StdMutex::new(Vec::new())),
        reasoning_route: ScriptedReasoningRoute::default(),
        fallback_to_b,
    })
}

/// The session a harness attaches: a fresh one (maybe seeded with rows) or
/// the retained session of an earlier attachment.
enum HarnessSession {
    Fresh(Session),
    Retained(SessionId),
}

async fn attach_harness(
    environment: Arc<AbEnvironment>,
    resume_session_id: Option<SessionId>,
) -> AbHarness {
    let session = match resume_session_id {
        Some(session_id) => HarnessSession::Retained(session_id),
        None => HarnessSession::Fresh(Session::new()),
    };
    try_attach_harness(environment, session)
        .await
        .expect("materialize the runtime-backed session with a real executor")
}

/// Attach `session` through the production composition, answering a
/// materialization failure instead of panicking on it.
async fn try_attach_harness(
    environment: Arc<AbEnvironment>,
    session: HarnessSession,
) -> Result<AbHarness, String> {
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
            reasoning_route: Arc::clone(&environment.reasoning_route),
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

    let mut config = ab_test_config(route, environment.availability.as_ref());
    if environment.fallback_to_b {
        config.model_fallback.enabled = Some(true);
        config.model_fallback.chain = vec![meerkat_core::config::ModelFallbackTarget {
            model: route.model_b.to_string(),
            provider: Some(route.provider_b),
            auth_binding: Some(expected_auth_binding_named(route.binding_b)),
        }];
    }
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
    )
    .expect("construct runtime authority");

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
        )
        .expect("compose the runtime-backed service");

    let fresh = matches!(session, HarnessSession::Fresh(_));
    let session = match session {
        HarnessSession::Retained(session_id) => service
            .load_authoritative_session(&session_id)
            .await
            .expect("load the retained authoritative session after restart")
            .expect("the retained session must remain durable"),
        HarnessSession::Fresh(session) => session,
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
    .map_err(|error| format!("{error:?}"))?;
    assert_eq!(materialized.session_id, session_id);

    Ok(AbHarness {
        service,
        adapter,
        runtime_store,
        session_id,
        runtime_id,
        environment,
    })
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

/// A cold restart must not depend on the teardown finishing inside the plain
/// unregister's caller grace. Under load the coordinator-owned teardown can
/// outlast that grace, and the grace-bounded unregister then answers the
/// retryable `UnregisterInProgress`. Here the teardown is held past a
/// shortened grace, which makes that window deterministic: the restart has
/// to wait for the held teardown and still come back on the same identity.
async fn cold_restart_outlasts_the_unregister_caller_grace(profile: PersistenceProfile) {
    let harness = build_harness(profile).await;
    let before = harness.durable_identity().await;
    harness
        .adapter
        .test_set_unregister_caller_wait_grace(std::time::Duration::from_millis(20));
    let (entered, release) = harness.adapter.test_hold_next_unregister_saga();
    let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let releaser = tokio::spawn({
        let released = Arc::clone(&released);
        async move {
            entered.await.expect("the teardown reaches its hold");
            // Release only well after the shortened caller grace elapsed.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            released.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = release.send(());
        }
    });

    let harness = harness.cold_restart().await;
    assert!(
        released.load(std::sync::atomic::Ordering::SeqCst),
        "the restart reattached only after the held teardown was released"
    );
    releaser.await.expect("the releaser finishes");
    assert_eq!(harness.durable_identity().await, before);
    assert_eq!(harness.live_identity().await.model, before.model);
}

#[tokio::test]
async fn whole_blob_cold_restart_outlasts_the_unregister_caller_grace() {
    cold_restart_outlasts_the_unregister_caller_grace(PersistenceProfile::WholeBlob).await;
}

#[tokio::test]
async fn head_canonical_cold_restart_outlasts_the_unregister_caller_grace() {
    cold_restart_outlasts_the_unregister_caller_grace(PersistenceProfile::HeadCanonical).await;
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

// ---------------------------------------------------------------------------
// A target that cannot carry the session's own operator-instruction entries
// ---------------------------------------------------------------------------

/// A session the factory built and persisted on `route`'s A, which had a
/// conversation before the harness attaches it: its durable session metadata
/// names A, as every resumed document carries it, and the operator-instruction
/// entry its next fresh run start applies stands after conversation rows.
fn conversed_seed(route: RouteSpec) -> Session {
    let mut session = Session::new();
    session
        .set_session_metadata(meerkat_core::SessionMetadata {
            model_fallback: None,
            schema_version: meerkat_core::SESSION_METADATA_SCHEMA_VERSION,
            model: route.model_a.to_string(),
            max_tokens: 8_192,
            structured_output_retries: 2,
            provider: route.provider_a,
            self_hosted_server_id: None,
            provider_params: None,
            tooling: meerkat_core::SessionTooling::default(),
            keep_alive: false,
            comms_name: None,
            peer_meta: None,
            realm_id: Some(RealmId::parse(TEST_REALM).expect("valid test realm")),
            instance_id: None,
            backend: None,
            config_generation: None,
            auth_binding: Some(expected_auth_binding_named(route.binding_a)),
            mob_member_binding: None,
        })
        .expect("durable session metadata");
    session.push(meerkat_core::Message::User(
        meerkat_core::UserMessage::text("an earlier question"),
    ));
    session.push(meerkat_core::Message::BlockAssistant(
        meerkat_core::BlockAssistantMessage {
            blocks: vec![meerkat_core::AssistantBlock::Text {
                text: "an earlier answer".to_string(),
                meta: None,
            }],
            stop_reason: Some(StopReason::EndTurn),
            identity: meerkat_core::types::TranscriptMessageIdentity::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
            assistant_message_id: None,
        },
    ));
    session
}

/// A seeded harness on [`TO_LEADING_PREFIX_ONLY_ROUTE`] whose authoring run
/// applied an operator-instruction entry after the conversation and
/// committed a `brain_swap` request for B.
async fn harness_with_an_entry_b_cannot_carry(
    profile: PersistenceProfile,
) -> (AbHarness, CommittedHandoffProof) {
    use meerkat_core::service::SessionServiceInstructionExt as _;

    let harness = build_harness_seeded(
        profile,
        TO_LEADING_PREFIX_ONLY_ROUTE,
        conversed_seed(TO_LEADING_PREFIX_ONLY_ROUTE),
    )
    .await;
    Arc::clone(&harness.service)
        .set_session_instruction(
            &harness.session_id,
            meerkat_core::SessionInstructionWrite {
                slot: meerkat_core::InstructionSlot {
                    namespace: meerkat_core::InstructionNamespace::new("app.example")
                        .expect("namespace"),
                    key: meerkat_core::InstructionKey::new("tone").expect("key"),
                },
                revision: meerkat_core::InstructionRevision::new(1).expect("revision"),
                change: meerkat_core::SessionInstructionChange::Activate {
                    body: "Answer formally.".to_string(),
                    content_sha256: None,
                },
            },
        )
        .await
        .expect("the session records the operator instruction");
    let proof = commit_brain_swap_request(&harness).await;
    assert_eq!(
        harness
            .authoritative_session()
            .await
            .own_operator_instruction_entry_placement()
            .expect("the transcript projects")
            .refusal(meerkat_core::InstructionLowering::LeadingPrefixOnly),
        Some(meerkat_core::InstructionEntryLoweringRefusal::AfterConversationRow),
        "the authoring run applied its entry after the conversation"
    );
    (harness, proof)
}

/// A committed `brain_swap` onto a model that cannot carry the session's own
/// operator-instruction entry where it stands (after the conversation) is
/// denied by the routing machine, typed, when the next input realizes it:
/// no client is built for B, the request ends Denied, and the next input
/// runs on A. A hold would leave that input, and every later one,
/// unattempted for good.
async fn a_swap_the_target_cannot_carry_is_denied_and_the_next_input_runs_on_a(
    profile: PersistenceProfile,
) {
    let (harness, proof) = harness_with_an_entry_b_cannot_carry(profile).await;
    let route = proof.route;
    harness.clear_client_build_models();

    let next = harness.run_prompt("continue on the current model").await;
    assert!(
        matches!(next, CompletionOutcome::Completed(ref run) if run.text == NEXT_TURN_TEXT),
        "the next input must run on A once the request is denied: {next:?}"
    );
    assert_eq!(
        harness.requested_calls().last(),
        Some(&ProviderCall {
            provider: route.provider_a,
            model: route.model_a.to_string(),
        }),
        "A serves the next input"
    );
    assert!(
        !harness
            .client_build_models()
            .iter()
            .any(|model| model == route.model_b),
        "no client is built for a target the routing machine denies: {:?}",
        harness.client_build_models()
    );
    let records = harness.durable_routing_records().await;
    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(records[1].request_id(), &proof.request_id);
    assert!(
        matches!(
            &records[1],
            SessionModelRoutingControlRecord::ModelRoutingIntentDenied {
                reason: meerkat_core::image_generation::SwitchTurnDenialReason::CapabilityPolicy,
                ..
            }
        ),
        "the request ends Denied, typed: {records:?}"
    );
    assert_eq!(harness.realized_terminal_count().await, 0);
    assert_eq!(harness.live_identity().await.model, route.model_a);
    assert_eq!(harness.durable_identity().await.model, route.model_a);
}

#[tokio::test]
async fn whole_blob_a_swap_the_target_cannot_carry_is_denied_and_the_next_input_runs_on_a() {
    a_swap_the_target_cannot_carry_is_denied_and_the_next_input_runs_on_a(
        PersistenceProfile::WholeBlob,
    )
    .await;
}

#[tokio::test]
async fn head_canonical_a_swap_the_target_cannot_carry_is_denied_and_the_next_input_runs_on_a() {
    a_swap_the_target_cannot_carry_is_denied_and_the_next_input_runs_on_a(
        PersistenceProfile::HeadCanonical,
    )
    .await;
}

/// A restart that cannot build A bootstraps a client for the committed
/// request's target only when that target can carry the session's own
/// operator-instruction entries where they stand. B cannot, so no client is
/// built for it and the restart fails on A's own error: a bootstrap client
/// for B would serve a transcript its wire refuses.
async fn a_restart_bootstraps_no_client_for_a_target_that_cannot_carry_own_entries(
    profile: PersistenceProfile,
) {
    let (harness, proof) = harness_with_an_entry_b_cannot_carry(profile).await;
    let route = proof.route;
    harness.set_available(route.model_a, false);
    harness.clear_client_build_models();
    let (environment, session_id) = harness.detach().await;

    let restarted = try_attach_harness(
        Arc::clone(&environment),
        HarnessSession::Retained(session_id),
    )
    .await;
    let built = environment
        .client_build_models
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert!(
        !built.iter().any(|model| model == route.model_b),
        "no bootstrap client is built for a target that cannot carry the entries: {built:?}"
    );
    assert!(
        restarted.is_err(),
        "with A unavailable and no bootstrap the restart fails on A's error"
    );
}

#[tokio::test]
async fn whole_blob_a_restart_bootstraps_no_client_for_a_target_that_cannot_carry_own_entries() {
    a_restart_bootstraps_no_client_for_a_target_that_cannot_carry_own_entries(
        PersistenceProfile::WholeBlob,
    )
    .await;
}

#[tokio::test]
async fn head_canonical_a_restart_bootstraps_no_client_for_a_target_that_cannot_carry_own_entries()
{
    a_restart_bootstraps_no_client_for_a_target_that_cannot_carry_own_entries(
        PersistenceProfile::HeadCanonical,
    )
    .await;
}

// ---------------------------------------------------------------------------
// The session's reasoning effort level
// ---------------------------------------------------------------------------

/// An account route whose model list states `levels` for the selected model.
fn account_route_stating(
    levels: &[meerkat_core::model_profile::capabilities::EffortLevel],
) -> meerkat_core::ReasoningRouteObservation {
    meerkat_core::ReasoningRouteObservation {
        backend: meerkat_core::ReasoningBackend::OpenAi(
            meerkat_core::provider_matrix::OpenAiBackendKind::Copilot,
        ),
        api: meerkat_core::ModelApi::Responses,
        account: meerkat_core::AccountLevelStatement::Stated(levels.to_vec()),
        operator_levels: None,
    }
}

fn level_request(
    model: Option<&str>,
    level: Option<
        meerkat_core::lifecycle::run_primitive::TurnMetadataOverride<
            meerkat_core::model_profile::capabilities::EffortLevel,
        >,
    >,
) -> meerkat_runtime::SessionLlmReconfigureRequest {
    meerkat_runtime::SessionLlmReconfigureRequest {
        model: model.map(str::to_string),
        provider: None,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
        reasoning_effort: level,
    }
}

/// A committed `brain_swap` handoff is not judged against the session's
/// reasoning effort level; only a caller's reconfiguration is.
///
/// The session takes a level its route accepts while a handoff to B is
/// committed. A caller's hot swap to B is refused typed (B's route does not
/// accept the level) and leaves the session whole. The handoff to the same
/// model is then realized at the next input as before, keeping the level, and
/// that input's request is refused before it reaches the provider. Clearing
/// the level makes the next request run on B.
#[tokio::test]
async fn a_brain_swap_keeps_the_level_and_requests_are_refused_until_it_is_cleared() {
    use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
    use meerkat_core::model_profile::capabilities::EffortLevel;

    let harness = build_harness(PersistenceProfile::WholeBlob).await;
    let route = harness.environment.route;
    let _committed = commit_brain_swap_request(&harness).await;

    // A's entry of the account's list states `high`: the session takes it.
    harness.set_reasoning_route(Some(account_route_stating(&[EffortLevel::High])));
    harness
        .adapter
        .reconfigure_session_llm_identity(
            &harness.session_id,
            level_request(None, Some(TurnMetadataOverride::Set(EffortLevel::High))),
        )
        .await
        .unwrap_or_else(|error| panic!("a level the route accepts is applied: {error}"));
    let before_durable = harness.durable_identity().await;
    let before_live = harness.live_identity().await;
    assert_eq!(before_live, before_durable);
    assert_eq!(before_live.model, route.model_a);
    assert_eq!(
        before_live
            .provider_params
            .as_ref()
            .and_then(|params| params.reasoning_effort),
        Some(EffortLevel::High)
    );

    // B's entry states only `low`. A caller's hot swap to B is refused typed
    // and changes nothing, live or stored.
    harness.set_reasoning_route(Some(account_route_stating(&[EffortLevel::Low])));
    let error = harness
        .adapter
        .reconfigure_session_llm_identity(
            &harness.session_id,
            level_request(Some(route.model_b), None),
        )
        .await
        .expect_err("a caller's swap to a route that refuses the level is refused");
    let meerkat_runtime::RuntimeDriverError::ReasoningEffortRefused(refusal) = &error else {
        panic!("expected the typed reasoning effort refusal, got: {error:?}");
    };
    assert_eq!(
        serde_json::to_value(&**refusal).expect("a refusal serializes"),
        serde_json::json!({
            "requested": "high", "provider": "openai", "model": route.model_b,
            "route": {"backend": "copilot", "api": "responses"},
            "reason": "unsupported_level", "supported": ["low"], "facts": "account",
        })
    );
    assert_eq!(harness.durable_identity().await, before_durable);
    assert_eq!(harness.live_identity().await, before_live);
    assert_eq!(harness.realized_terminal_count().await, 0);
    let calls_before = harness.requested_models();
    assert_eq!(
        calls_before,
        vec![route.model_a.to_string(), route.model_a.to_string()]
    );

    // The committed handoff to the same model is realized at the next input,
    // not judged: the session moves to B and keeps its level. The input's
    // own request is then refused before any provider call.
    let refused = harness.run_prompt("the next input, after the swap").await;
    assert_eq!(
        harness.realized_terminal_count().await,
        1,
        "the handoff is realized as before: {refused:?}"
    );
    let swapped = harness.live_identity().await;
    assert_eq!(swapped.model, route.model_b);
    assert_eq!(
        swapped
            .provider_params
            .as_ref()
            .and_then(|params| params.reasoning_effort),
        Some(EffortLevel::High),
        "the handoff keeps the session's level"
    );
    assert_eq!(harness.durable_identity().await, swapped);
    assert_eq!(
        harness.requested_models(),
        calls_before,
        "the refused request reached no provider"
    );
    // The per-request check refuses it typed and names the level and the
    // route: no other outcome passes.
    let CompletionOutcome::AbandonedWithError { error, .. } = &refused else {
        panic!("a request whose level the route refuses must fail typed: {refused:?}");
    };
    let Some(meerkat_core::event::AgentErrorReason::ReasoningEffortRefused { refusal }) =
        &error.reason
    else {
        panic!("the request must be refused for its level, typed: {refused:?}");
    };
    assert_eq!(refusal.requested, EffortLevel::High);
    assert_eq!(refusal.model, route.model_b);
    assert_eq!(
        refusal.route.as_ref().map(ToString::to_string).as_deref(),
        Some("copilot/responses")
    );

    // Clearing the level makes the next request run, on B.
    harness
        .adapter
        .reconfigure_session_llm_identity(
            &harness.session_id,
            level_request(None, Some(TurnMetadataOverride::Clear)),
        )
        .await
        .unwrap_or_else(|error| panic!("clearing the level is applied: {error}"));
    let next = harness
        .run_prompt("the input after the level is cleared")
        .await;
    assert!(
        matches!(next, CompletionOutcome::Completed(ref run) if run.text == NEXT_TURN_TEXT),
        "the request runs once the level is cleared: {next:?}"
    );
    assert_eq!(
        harness.requested_calls().last(),
        Some(&ProviderCall {
            provider: route.provider_b,
            model: route.model_b.to_string(),
        })
    );
    assert_eq!(
        harness
            .live_identity()
            .await
            .provider_params
            .and_then(|params| params.reasoning_effort),
        None
    );
}

/// A session's fallback client is built for the level the session holds at
/// that build: its chain target carries that level. A caller's change of the
/// level, and a clear, each build the session's own client alone, so no
/// fallback client, and no chain target that carries the earlier level,
/// outlives the change. The session's next build (here a cold restart)
/// builds the chain target again, for the level the session holds then.
#[tokio::test]
async fn a_level_change_or_clear_replaces_the_fallback_client() {
    use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
    use meerkat_core::model_profile::capabilities::EffortLevel;

    let route = SAME_PROVIDER_ROUTE;
    let mut harness = attach_harness(
        new_environment_with(PersistenceProfile::WholeBlob, route, true),
        None,
    )
    .await;
    // The account's list states both levels for every model of the route.
    harness.set_reasoning_route(Some(account_route_stating(&[
        EffortLevel::Low,
        EffortLevel::High,
    ])));

    for (step, level) in [
        ("set", Some(EffortLevel::High)),
        ("change", Some(EffortLevel::Low)),
        ("clear", None),
    ] {
        // The session as it was built: its own client and the chain
        // target's, the fallback client's two candidates.
        let built = harness.client_build_models();
        assert!(
            built.iter().any(|model| model == route.model_a)
                && built.iter().any(|model| model == route.model_b),
            "{step}: a session build builds the fallback chain target too: {built:?}"
        );

        harness.clear_client_build_models();
        let change = match level {
            Some(level) => TurnMetadataOverride::Set(level),
            None => TurnMetadataOverride::Clear,
        };
        harness
            .adapter
            .reconfigure_session_llm_identity(
                &harness.session_id,
                level_request(None, Some(change)),
            )
            .await
            .unwrap_or_else(|error| panic!("{step}: the level change is applied: {error}"));
        let rebuilt = harness.client_build_models();
        assert!(
            !rebuilt.is_empty() && rebuilt.iter().all(|model| model == route.model_a),
            "{step}: the change builds the session's own client alone, no chain target: \
             {rebuilt:?}"
        );
        assert_eq!(
            harness
                .live_identity()
                .await
                .provider_params
                .and_then(|params| params.reasoning_effort),
            level,
            "{step}: the session holds the new level"
        );

        // The session's next build is for the level it holds now.
        harness.clear_client_build_models();
        harness = harness.cold_restart().await;
    }
}

/// The model-routing history is stored in the session's head row, and an
/// earlier release decodes the head rows of a realm typed when it looks a
/// session up; its provider params have no member for the provider-neutral
/// reasoning effort level. So the realized record of a `brain_swap` holds
/// the applied identity WITHOUT the level: it names the model that was
/// installed, and the whole stored head row names no level. The session's
/// own settings keep the level, live, stored and after a cold restart. A
/// record in the earlier form, with the level inside, still decodes.
#[tokio::test]
async fn a_brain_swap_under_a_level_records_its_applied_identity_without_the_level() {
    use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
    use meerkat_core::model_profile::capabilities::EffortLevel;
    use meerkat_core::session_store::IncrementalSessionStore as _;

    let level_of = |identity: &SessionLlmIdentity| {
        identity
            .provider_params
            .as_ref()
            .and_then(|params| params.reasoning_effort)
    };
    let harness = build_harness(PersistenceProfile::HeadCanonical).await;
    let route = harness.environment.route;
    let _committed = commit_brain_swap_request(&harness).await;

    // Both models' entries of the account's list state `high`: the session
    // takes the level, and the request after the switch runs at it.
    harness.set_reasoning_route(Some(account_route_stating(&[EffortLevel::High])));
    harness
        .adapter
        .reconfigure_session_llm_identity(
            &harness.session_id,
            level_request(None, Some(TurnMetadataOverride::Set(EffortLevel::High))),
        )
        .await
        .unwrap_or_else(|error| panic!("a level the route accepts is applied: {error}"));
    assert_eq!(
        level_of(&harness.live_identity().await),
        Some(EffortLevel::High)
    );

    // The committed handoff is realized at the next input.
    let completion = harness.run_prompt("the next input, after the swap").await;
    assert_eq!(
        harness.realized_terminal_count().await,
        1,
        "the handoff is realized: {completion:?}"
    );
    let swapped = harness.live_identity().await;
    assert_eq!(swapped.model, route.model_b);
    assert_eq!(
        level_of(&swapped),
        Some(EffortLevel::High),
        "the session keeps its level across the switch"
    );
    assert_eq!(harness.durable_identity().await, swapped);

    // The realized record: the installed model, and no level.
    let records = harness.durable_routing_records().await;
    let realized: Vec<&SessionLlmIdentity> = records
        .iter()
        .filter_map(|record| match record {
            SessionModelRoutingControlRecord::ModelRoutingIntentRealized {
                applied_identity,
                ..
            } => Some(&**applied_identity),
            _ => None,
        })
        .collect();
    assert_eq!(realized.len(), 1, "one realized record: {records:?}");
    assert_eq!(realized[0].model, route.model_b);
    assert_eq!(
        level_of(realized[0]),
        None,
        "the routing history records the applied identity without the level"
    );
    let stored_records = serde_json::to_string(&records).expect("the stored records");
    assert!(
        !stored_records.contains("reasoning_effort"),
        "the stored routing history names no level: {stored_records}"
    );

    // The whole head row, as the session store holds it: what an earlier
    // release decodes when it looks a session of the realm up.
    let session_store = meerkat_store::SqliteSessionStore::open(
        harness.environment.temp.path().join("sessions.sqlite3"),
    )
    .expect("open the sqlite session store");
    let head = session_store
        .load_head(&harness.session_id)
        .await
        .expect("load the stored head")
        .expect("the session has a head row");
    let stored_head = serde_json::to_string(&head).expect("the stored head row's form");
    assert!(
        stored_head.contains("model_routing_intent_realized"),
        "the head row holds the realized record: {stored_head}"
    );
    assert!(
        !stored_head.contains("reasoning_effort"),
        "the stored head row names no level: {stored_head}"
    );

    // A record in the earlier form, with the level inside, still decodes.
    let mut earlier = serde_json::to_value(&records).expect("the records as a value");
    let earlier_record = earlier
        .as_array_mut()
        .expect("the records are a list")
        .iter_mut()
        .find(|record| record["record"] == "model_routing_intent_realized")
        .expect("the realized record");
    earlier_record["applied_identity"]["provider_params"] =
        serde_json::json!({"reasoning_effort": "high"});
    let decoded: Vec<SessionModelRoutingControlRecord> =
        serde_json::from_value(earlier).expect("a record with the level inside decodes");
    assert!(decoded.iter().any(|record| matches!(
        record,
        SessionModelRoutingControlRecord::ModelRoutingIntentRealized { applied_identity, .. }
            if level_of(applied_identity) == Some(EffortLevel::High)
    )));

    // After a cold restart the session still holds its level.
    let harness = harness.cold_restart().await;
    assert_eq!(
        level_of(&harness.durable_identity().await),
        Some(EffortLevel::High),
        "the session's own settings keep the level"
    );
}

// ---------------------------------------------------------------------------
// The rollback fixture for the session's head row
// ---------------------------------------------------------------------------

/// The JSON shape of a stored value as the previous release decodes it.
enum Shape {
    /// The previous release keeps this value untyped.
    Any,
    /// A string, a number, a boolean or null.
    Leaf,
    /// An object with exactly these members.
    Object(&'static [(&'static str, Shape)]),
    /// A list of values of one shape.
    List(&'static Shape),
    /// An object whose member `.0` names one of the variants `.1`, each with
    /// its own members.
    Tagged(
        &'static str,
        &'static [(&'static str, &'static [(&'static str, Shape)])],
    ),
}

const ROW_PREFIX: Shape = Shape::Object(&[
    ("version", Shape::Leaf),
    ("row_count", Shape::Leaf),
    ("digest", Shape::Leaf),
]);
const REWRITE_PREFIX: Shape =
    Shape::Object(&[("occurrence_count", Shape::Leaf), ("digest", Shape::Leaf)]);
const SYSTEM_TIME: Shape = Shape::Object(&[
    ("secs_since_epoch", Shape::Leaf),
    ("nanos_since_epoch", Shape::Leaf),
]);
const REASON_DISPOSITION: Shape = Shape::Tagged(
    "disposition",
    &[
        ("not_provided", &[]),
        (
            "provided",
            &[("reason", Shape::Object(&[("content", Shape::Leaf)]))],
        ),
    ],
);
const SWITCH_INTENT: Shape = Shape::Object(&[
    ("target_model", Shape::Leaf),
    (
        "duration",
        Shape::Tagged(
            "duration_type",
            &[
                (
                    "finite",
                    &[(
                        "duration",
                        Shape::Tagged(
                            "duration",
                            &[("one_turn", &[]), ("turns", &[("turns", Shape::Leaf)])],
                        ),
                    )],
                ),
                ("until_changed", &[]),
            ],
        ),
    ),
    (
        "origin",
        Shape::Tagged(
            "origin",
            &[
                ("user", &[("reason", REASON_DISPOSITION)]),
                ("model", &[("reason", REASON_DISPOSITION)]),
                ("system_policy", &[("reason", Shape::Leaf)]),
            ],
        ),
    ),
]);
/// A provider's own tag: the member names of each provider's tag at the
/// previous release; their values are that provider's own settings.
const PROVIDER_TAG: Shape = Shape::Tagged(
    "provider",
    &[
        (
            "anthropic",
            &[
                ("thinking", Shape::Any),
                ("thinking_budget_tokens", Shape::Leaf),
                ("web_search", Shape::Leaf),
                ("top_k", Shape::Leaf),
                ("effort", Shape::Leaf),
                ("structured_output", Shape::Any),
                ("inference_geo", Shape::Any),
                ("compaction", Shape::Any),
                ("context", Shape::Leaf),
                ("cache_control", Shape::Leaf),
                ("cache_ttl", Shape::Leaf),
                ("supports_temperature_override", Shape::Leaf),
            ],
        ),
        (
            "open_ai",
            &[
                ("reasoning_effort", Shape::Leaf),
                ("reasoning_mode", Shape::Leaf),
                ("reasoning_context", Shape::Leaf),
                ("text_verbosity", Shape::Leaf),
                ("seed", Shape::Leaf),
                ("frequency_penalty", Shape::Leaf),
                ("presence_penalty", Shape::Leaf),
                ("web_search", Shape::Leaf),
                ("structured_output", Shape::Any),
                ("reasoning", Shape::Leaf),
                ("chat_template_kwargs", Shape::Leaf),
                ("thinking", Shape::Leaf),
                ("store", Shape::Leaf),
                ("prompt_cache_enabled", Shape::Leaf),
                ("prompt_cache_key", Shape::Leaf),
                ("prompt_cache_retention", Shape::Leaf),
                (
                    "prompt_cache_options",
                    Shape::Object(&[("mode", Shape::Leaf), ("ttl", Shape::Leaf)]),
                ),
                ("supports_temperature_override", Shape::Leaf),
                ("supports_reasoning_override", Shape::Leaf),
            ],
        ),
        (
            "gemini",
            &[
                (
                    "thinking",
                    Shape::Object(&[
                        ("include_thoughts", Shape::Leaf),
                        ("thinking_level", Shape::Leaf),
                        ("thinking_budget", Shape::Leaf),
                    ]),
                ),
                ("thinking_budget", Shape::Leaf),
                ("thinking_level", Shape::Leaf),
                ("top_k", Shape::Leaf),
                ("top_p", Shape::Leaf),
                ("structured_output", Shape::Any),
                ("google_search", Shape::Leaf),
                ("candidate_count", Shape::Leaf),
                ("cached_content_name", Shape::Leaf),
            ],
        ),
        ("unknown", &[("bag", Shape::Any)]),
    ],
);
/// The identity a realized routing record holds. Its provider params are
/// the previous release's: six members, and no provider-neutral
/// `reasoning_effort`.
const RECORDED_IDENTITY: Shape = Shape::Object(&[
    ("model", Shape::Leaf),
    ("provider", Shape::Leaf),
    ("self_hosted_server_id", Shape::Leaf),
    (
        "provider_params",
        Shape::Object(&[
            ("temperature", Shape::Leaf),
            ("top_p", Shape::Leaf),
            ("max_output_tokens", Shape::Leaf),
            ("reasoning", Shape::Leaf),
            ("thinking_budget_tokens", Shape::Leaf),
            ("provider_tag", PROVIDER_TAG),
        ]),
    ),
    (
        "auth_binding",
        Shape::Object(&[
            ("realm", Shape::Leaf),
            ("binding", Shape::Leaf),
            ("profile", Shape::Leaf),
            ("origin", Shape::Leaf),
        ]),
    ),
]);
const ROUTING_RECORD: Shape = Shape::Tagged(
    "record",
    &[
        (
            "model_routing_intent_requested",
            &[
                ("request_id", Shape::Leaf),
                ("originating_run_id", Shape::Leaf),
                ("intent", SWITCH_INTENT),
            ],
        ),
        (
            "model_routing_intent_realized",
            &[
                ("request_id", Shape::Leaf),
                ("originating_run_id", Shape::Leaf),
                ("intent", SWITCH_INTENT),
                ("applied_identity", RECORDED_IDENTITY),
            ],
        ),
        (
            "model_routing_intent_denied",
            &[
                ("request_id", Shape::Leaf),
                ("originating_run_id", Shape::Leaf),
                ("intent", SWITCH_INTENT),
                (
                    "reason",
                    Shape::Tagged(
                        "reason",
                        &[
                            ("unsupported_model", &[]),
                            ("capability_policy", &[]),
                            ("cost_policy", &[]),
                            ("safety_policy", &[]),
                            ("approval_required_but_unavailable", &[]),
                            ("scoped_override_conflict", &[]),
                            ("realtime_transport_conflict", &[]),
                            ("projection_unsupported", &[]),
                            ("denied_during_approval", &[("approvable", Shape::Leaf)]),
                        ],
                    ),
                ),
            ],
        ),
        (
            "model_routing_intent_abandoned",
            &[
                ("request_id", Shape::Leaf),
                ("originating_run_id", Shape::Leaf),
                ("intent", SWITCH_INTENT),
            ],
        ),
    ],
);
/// THE ROLLBACK FIXTURE of the session's head row: the head's shape as
/// 0.9.14 decodes it, record by record and member by member, copied from
/// that release's types (`SessionHead` and what it reaches in
/// `meerkat-core`: the row and rewrite prefix accumulators, the lineage
/// anchor, the graph prefix, the realtime event prefix, the metadata
/// identity, `Usage` with its provider accounting, and the model-routing
/// records with their intent and applied identity). 0.9.14 decodes the head
/// rows of a realm typed when it looks a session up, and it recomputes each
/// head's token from what it decoded: a member it does not know either
/// fails the decode or changes the token, and both fail the lookup for the
/// whole realm. A change that adds a member or a value to anything the head
/// row holds fails here until the row stays readable by the previous
/// release (the rule is written at `RunResult::reasoning_effort` and at
/// `InputTerminalCompletion::outcome` for the input row; this is its
/// counterpart for the head row).
const TAG_HEAD_SHAPE: Shape = Shape::Object(&[
    ("id", Shape::Leaf),
    ("version", Shape::Leaf),
    ("strand", Shape::Leaf),
    ("head_revision", Shape::Leaf),
    ("message_count", Shape::Leaf),
    ("message_row_prefix", ROW_PREFIX),
    (
        "row_lineage_anchor",
        Shape::Object(&[
            ("rewrite_count", Shape::Leaf),
            ("rewrite_prefix", REWRITE_PREFIX),
            ("strand", Shape::Leaf),
            ("message_count", Shape::Leaf),
            ("materialized_prefix", ROW_PREFIX),
            ("prefix", ROW_PREFIX),
        ]),
    ),
    ("rewrite_count", Shape::Leaf),
    ("rewrite_prefix", REWRITE_PREFIX),
    (
        "graph_prefix",
        Shape::Object(&[("occurrence_count", Shape::Leaf), ("digest", Shape::Leaf)]),
    ),
    (
        "realtime_event_prefix",
        Shape::Object(&[
            ("version", Shape::Leaf),
            ("session_id", Shape::Leaf),
            ("component", Shape::Leaf),
            ("event_count", Shape::Leaf),
            ("root_digest", Shape::Leaf),
        ]),
    ),
    ("created_at", SYSTEM_TIME),
    ("updated_at", SYSTEM_TIME),
    (
        "usage",
        Shape::Object(&[
            ("input_tokens", Shape::Leaf),
            ("output_tokens", Shape::Leaf),
            ("cache_creation_tokens", Shape::Leaf),
            ("cache_read_tokens", Shape::Leaf),
            ("reasoning_tokens", Shape::Leaf),
            (
                "provider_accounting",
                Shape::Object(&[
                    ("provider", Shape::Leaf),
                    ("model", Shape::Leaf),
                    ("presented_tokens", Shape::Leaf),
                    ("convention", Shape::Leaf),
                    ("aggregation", Shape::Leaf),
                ]),
            ),
        ]),
    ),
    (
        "metadata_identity",
        Shape::Object(&[
            ("format_version", Shape::Leaf),
            ("entry_count", Shape::Leaf),
            ("exact_root_digest", Shape::Leaf),
        ]),
    ),
    // The previous release keeps the inline metadata map untyped.
    ("metadata", Shape::Any),
    ("model_routing_control", Shape::List(&ROUTING_RECORD)),
]);

/// Every place under `value` that `shape` does not know, as a path.
fn unknown_to_shape(value: &serde_json::Value, shape: &Shape, path: &str, found: &mut Vec<String>) {
    let members = |object: &serde_json::Map<String, serde_json::Value>,
                   fields: &[(&'static str, Shape)],
                   tag: Option<&str>,
                   found: &mut Vec<String>| {
        for (key, member) in object {
            if tag == Some(key.as_str()) {
                continue;
            }
            match fields.iter().find(|(name, _)| name == key) {
                Some((_, member_shape)) => {
                    unknown_to_shape(member, member_shape, &format!("{path}.{key}"), found);
                }
                None => found.push(format!("{path}.{key}")),
            }
        }
    };
    match (shape, value) {
        (Shape::Any, _) | (_, serde_json::Value::Null) => {}
        (Shape::Leaf, serde_json::Value::Object(_) | serde_json::Value::Array(_)) => {
            found.push(format!("{path}: a structure where a plain value is read"));
        }
        (Shape::Leaf, _) => {}
        (Shape::Object(fields), serde_json::Value::Object(object)) => {
            members(object, fields, None, found);
        }
        (Shape::List(item), serde_json::Value::Array(items)) => {
            for (index, member) in items.iter().enumerate() {
                unknown_to_shape(member, item, &format!("{path}[{index}]"), found);
            }
        }
        (Shape::Tagged(tag, variants), serde_json::Value::Object(object)) => {
            let named = object.get(*tag).and_then(serde_json::Value::as_str);
            match variants.iter().find(|(name, _)| Some(*name) == named) {
                Some((_, fields)) => members(object, fields, Some(tag), found),
                None => found.push(format!("{path}.{tag} = {named:?}")),
            }
        }
        (_, other) => found.push(format!("{path}: {other} where another form is read")),
    }
}

/// The stored head row of `session_id` in the SQLite sessions store at
/// `store_path` holds nothing the previous release's head does not know.
/// `step` names where the session is in its life.
pub(super) async fn assert_the_stored_head_has_the_previous_releases_shape(
    store_path: &std::path::Path,
    session_id: &SessionId,
    step: &str,
) {
    use meerkat_core::session_store::IncrementalSessionStore as _;

    let store = meerkat_store::SqliteSessionStore::open(store_path.to_path_buf())
        .expect("open the sqlite session store");
    let head = store
        .load_head(session_id)
        .await
        .expect("load the stored head")
        .unwrap_or_else(|| panic!("{step}: the session has a head row"));
    let stored = serde_json::to_value(&head).expect("the stored head row's form");
    let mut found = Vec::new();
    unknown_to_shape(&stored, &TAG_HEAD_SHAPE, "head", &mut found);
    assert!(
        found.is_empty(),
        "{step}: the stored head row holds what the previous release's head does not know: \
         {found:?} in {stored}"
    );
}

/// The fixture refuses what it must: a head whose realized routing record
/// carries the provider-neutral level, and a head with a member of its own
/// that the previous release does not have.
#[test]
fn the_head_shape_fixture_refuses_a_level_in_a_routing_record_and_an_unknown_member() {
    let identity = serde_json::json!({
        "model": "model-b", "provider": "openai",
        "provider_params": {"reasoning_effort": "high"},
    });
    let head = |params: serde_json::Value, extra: Option<&str>| {
        let mut identity = identity.clone();
        identity["provider_params"] = params;
        let mut head = serde_json::json!({
            "id": "018f7b9a-1c2d-7e3f-8a4b-5c6d7e8f9a0b", "version": 1, "strand": "root",
            "head_revision": "r", "message_count": 1, "rewrite_count": 0,
            "created_at": {"secs_since_epoch": 1, "nanos_since_epoch": 0},
            "updated_at": {"secs_since_epoch": 1, "nanos_since_epoch": 0},
            "usage": {"input_tokens": 1, "output_tokens": 1,
                      "cache_creation_tokens": null, "cache_read_tokens": null},
            "metadata": {"anything": {"reasoning_effort": "high"}},
            "model_routing_control": [{
                "record": "model_routing_intent_realized",
                "request_id": "018f7b9a-1c2d-7e3f-8a4b-5c6d7e8f9a0c",
                "originating_run_id": "018f7b9a-1c2d-7e3f-8a4b-5c6d7e8f9a0d",
                "intent": {"target_model": "model-b",
                           "duration": {"duration_type": "until_changed"},
                           "origin": {"origin": "model",
                                      "reason": {"disposition": "not_provided"}}},
                "applied_identity": identity,
            }],
        });
        if let Some(member) = extra {
            head[member] = serde_json::json!(true);
        }
        head
    };
    let unknown = |head: &serde_json::Value| {
        let mut found = Vec::new();
        unknown_to_shape(head, &TAG_HEAD_SHAPE, "head", &mut found);
        found
    };
    assert_eq!(
        unknown(&head(
            serde_json::json!({"provider_tag": {"provider": "open_ai", "reasoning_effort": "high"}}),
            None
        )),
        Vec::<String>::new(),
        "a provider's own effort and an untyped metadata map are known"
    );
    assert_eq!(
        unknown(&head(serde_json::json!({"reasoning_effort": "high"}), None)),
        vec![
            "head.model_routing_control[0].applied_identity.provider_params.reasoning_effort"
                .to_string()
        ]
    );
    assert_eq!(
        unknown(&head(
            serde_json::json!({}),
            Some("a_member_of_a_later_release")
        )),
        vec!["head.a_member_of_a_later_release".to_string()]
    );
}

/// The level's paths through the production SQLite store, with the stored
/// head row checked against the previous release's shape after each: a
/// session without a level; a level set on it (the reconfiguration a turn's
/// override makes); a turn at the level; the run's own permanent model
/// switch under the level; the level cleared; a cold restart. (A session
/// created with a level, and an ordinary model fallback under it, are
/// checked the same way by
/// `a_skipped_fallback_target_under_a_level_stores_its_reason_in_no_input_row`.)
#[tokio::test]
async fn the_stored_head_row_keeps_the_previous_releases_shape_on_every_level_path() {
    use meerkat_core::lifecycle::run_primitive::TurnMetadataOverride;
    use meerkat_core::model_profile::capabilities::EffortLevel;

    let harness = build_harness(PersistenceProfile::HeadCanonical).await;
    let store_path = harness.environment.temp.path().join("sessions.sqlite3");
    let session_id = harness.session_id.clone();
    let check = |step: &'static str| {
        let store_path = store_path.clone();
        let session_id = session_id.clone();
        async move {
            assert_the_stored_head_has_the_previous_releases_shape(&store_path, &session_id, step)
                .await;
        }
    };
    check("a session without a level").await;

    // The authoring turn commits the switch request (a requested record).
    let _committed = commit_brain_swap_request(&harness).await;
    check("after the turn that asks for the switch").await;

    harness.set_reasoning_route(Some(account_route_stating(&[EffortLevel::High])));
    harness
        .adapter
        .reconfigure_session_llm_identity(
            &harness.session_id,
            level_request(None, Some(TurnMetadataOverride::Set(EffortLevel::High))),
        )
        .await
        .unwrap_or_else(|error| panic!("a level the route accepts is applied: {error}"));
    check("a level set on the session").await;

    // A turn at the level, which also realizes the permanent switch.
    let completion = harness.run_prompt("the next input, after the swap").await;
    assert_eq!(
        harness.realized_terminal_count().await,
        1,
        "the handoff is realized: {completion:?}"
    );
    assert_eq!(
        harness
            .live_identity()
            .await
            .provider_params
            .and_then(|params| params.reasoning_effort),
        Some(EffortLevel::High)
    );
    check("a turn at the level and the run's own permanent switch").await;

    harness
        .adapter
        .reconfigure_session_llm_identity(
            &harness.session_id,
            level_request(None, Some(TurnMetadataOverride::Clear)),
        )
        .await
        .unwrap_or_else(|error| panic!("clearing the level is applied: {error}"));
    check("the level cleared").await;

    let harness = harness.cold_restart().await;
    assert_the_stored_head_has_the_previous_releases_shape(
        &harness.environment.temp.path().join("sessions.sqlite3"),
        &harness.session_id,
        "a cold restart",
    )
    .await;
}
