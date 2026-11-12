//! The provider-neutral `reasoning_effort` level on every route, through a
//! real agent loop.
//!
//! Each row builds the facade's agent on the real provider client of a route
//! and runs it against a loopback stub that records every request. The
//! routes are the three direct provider APIs, a self-hosted server on both
//! of its APIs, and the Copilot backend of each provider family. No row
//! sends a request anywhere but the stub; the Copilot account and its model
//! list are hand-written, generic stand-ins.
//!
//! What the rows pin, for every route:
//!
//! - a level the route's facts admit reaches the provider in the route's own
//!   field on every request of the run (a tool call round trip, streamed
//!   text, a structured-output request), and the run reports it;
//! - a level the facts do not admit, and a level nothing states the facts
//!   for, is refused with the typed refusal before any request is sent;
//! - a route that changes between the resolution of a level and the
//!   preparation of its request is resolved again, never sent with the
//!   earlier route's field.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::too_many_lines
)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    Router,
    extract::State,
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
};
use meerkat::{
    AgentBuilder, AgentFactory, AgentToolDispatcher, BuildAgentError, DynAgent, ToolDef, ToolError,
};
use meerkat_core::event::AgentErrorReason;
use meerkat_core::lifecycle::run_primitive::{
    AnthropicProviderTag, AnthropicThinkingConfig, ProviderParamsOverride, ProviderTag,
};
use meerkat_core::model_profile::capabilities::EffortLevel;
use meerkat_core::provider_matrix::{
    AnthropicBackendKind, GoogleBackendKind, OpenAiBackendKind, SelfHostedBackendKind,
};
use meerkat_core::{
    AgentError, AgentEvent, Config, ModelApi, OutputSchema, Provider, ReasoningBackend,
    ReasoningEffortDecidedBy, ReasoningEffortRefusal, ReasoningEffortRefusalReason,
    ReasoningEffortResolution, ReasoningFactsSource, ReasoningLevelFacts, ReasoningRoute,
    ReasoningRouteObservation, RetryPolicy, Session, ToolCallView, ToolDispatchOutcome, ToolResult,
};
use meerkat_llm_core::{
    LlmClient, LlmError, LlmReplayProjection, LlmRequest, LlmStream, PreparedLlmRequest,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

#[path = "support/test_session_store.rs"]
mod test_session_store;
use test_session_store::TestSessionStore;

// ---------------------------------------------------------------------------
// The stub: one loopback server for every provider API
// ---------------------------------------------------------------------------

const TOOL: &str = "lookup";
const ANSWER: [&str; 2] = ["The lookup ", "is done."];
const STRUCTURED: &str = r#"{"answer":"done"}"#;

/// What the stub answers one request with.
#[derive(Clone, Copy, Debug)]
enum Reply {
    /// One call of [`TOOL`].
    ToolCall,
    /// [`ANSWER`], streamed in two parts.
    Text,
    /// [`STRUCTURED`], the reply to a structured-output request.
    Json,
}

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    body: Value,
}

#[derive(Clone)]
struct StubState {
    script: Arc<Vec<Reply>>,
    recorded: Arc<Mutex<Vec<Recorded>>>,
}

struct Stub {
    base: String,
    recorded: Arc<Mutex<Vec<Recorded>>>,
    server: tokio::task::JoinHandle<()>,
}

impl Stub {
    /// Serve `script` in order (the last reply repeats) on every provider
    /// API path, recording each request's path and body.
    async fn spawn(script: &[Reply]) -> Self {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new().fallback(serve).with_state(StubState {
            script: Arc::new(script.to_vec()),
            recorded: Arc::clone(&recorded),
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind stub");
        let addr = listener.local_addr().expect("stub addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve stub");
        });
        Self {
            base: format!("http://{addr}"),
            recorded,
            server,
        }
    }

    fn recorded(&self) -> Vec<Recorded> {
        self.recorded.lock().unwrap().clone()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// The provider API a request path belongs to.
fn api_of_path(path: &str) -> Option<ModelApi> {
    if path.ends_with("/chat/completions") {
        Some(ModelApi::ChatCompletions)
    } else if path.ends_with("/responses") {
        Some(ModelApi::Responses)
    } else if path.ends_with("/messages") {
        Some(ModelApi::Messages)
    } else if path.contains(":streamGenerateContent") {
        Some(ModelApi::GenerateContent)
    } else {
        None
    }
}

async fn serve(State(state): State<StubState>, uri: Uri, body: String) -> Response {
    let path = uri.path().to_string();
    let Some(api) = api_of_path(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let body = serde_json::from_str(&body).unwrap_or(Value::Null);
    let index = {
        let mut recorded = state.recorded.lock().unwrap();
        recorded.push(Recorded { path, body });
        recorded.len() - 1
    };
    let reply = state
        .script
        .get(index)
        .or_else(|| state.script.last())
        .copied()
        .unwrap_or(Reply::Text);
    ([("content-type", "text/event-stream")], render(api, reply)).into_response()
}

/// `data:` lines, each followed by `separator`.
fn sse(separator: &str, data: &[Value]) -> String {
    data.iter()
        .map(|value| format!("data: {value}{separator}"))
        .collect()
}

/// The text parts a reply streams.
fn parts(reply: Reply) -> Vec<&'static str> {
    match reply {
        Reply::ToolCall => Vec::new(),
        Reply::Text => ANSWER.to_vec(),
        Reply::Json => vec![STRUCTURED],
    }
}

/// One scripted reply in the stream format of `api`.
fn render(api: ModelApi, reply: Reply) -> String {
    let parts = parts(reply);
    match api {
        ModelApi::Responses => {
            let usage = json!({"input_tokens": 10, "output_tokens": 3, "total_tokens": 13});
            let mut data: Vec<Value> = parts
                .iter()
                .map(|part| json!({"type": "response.output_text.delta", "delta": part}))
                .collect();
            let output = match reply {
                Reply::ToolCall => json!([{
                    "type": "function_call", "id": "fc_1", "call_id": "call_1",
                    "name": TOOL, "arguments": "{}", "status": "completed"
                }]),
                Reply::Text | Reply::Json => json!([]),
            };
            data.push(json!({
                "type": "response.completed",
                "response": {
                    "id": "resp_1", "status": "completed", "output": output, "usage": usage
                }
            }));
            let mut body = sse("\n\n", &data);
            body.push_str("data: [DONE]\n\n");
            body
        }
        ModelApi::ChatCompletions => {
            let mut data: Vec<Value> = parts
                .iter()
                .map(|part| {
                    json!({"id": "c1", "choices": [{
                        "index": 0, "delta": {"content": part}, "finish_reason": null
                    }]})
                })
                .collect();
            let finish_reason = match reply {
                Reply::ToolCall => {
                    data.push(json!({"id": "c1", "choices": [{
                        "index": 0,
                        "delta": {"tool_calls": [{
                            "index": 0, "id": "call_1", "type": "function",
                            "function": {"name": TOOL, "arguments": "{}"}
                        }]},
                        "finish_reason": null
                    }]}));
                    "tool_calls"
                }
                Reply::Text | Reply::Json => "stop",
            };
            data.push(json!({
                "id": "c1",
                "choices": [{"index": 0, "delta": {}, "finish_reason": finish_reason}],
                "usage": {"prompt_tokens": 4, "completion_tokens": 1}
            }));
            let mut body = sse("\n\n", &data);
            body.push_str("data: [DONE]\n\n");
            body
        }
        ModelApi::Messages => {
            let mut data = vec![json!({
                "type": "message_start",
                "message": {"usage": {"input_tokens": 40, "output_tokens": 0}}
            })];
            let stop_reason = match reply {
                Reply::ToolCall => {
                    data.push(json!({
                        "type": "content_block_start",
                        "content_block": {"type": "tool_use", "id": "toolu_1", "name": TOOL}
                    }));
                    data.push(json!({
                        "type": "content_block_delta",
                        "delta": {"type": "input_json_delta", "partial_json": "{}"}
                    }));
                    "tool_use"
                }
                Reply::Text | Reply::Json => {
                    data.push(json!({
                        "type": "content_block_start",
                        "content_block": {"type": "text", "text": ""}
                    }));
                    for part in &parts {
                        data.push(json!({
                            "type": "content_block_delta",
                            "delta": {"type": "text_delta", "text": part}
                        }));
                    }
                    "end_turn"
                }
            };
            data.push(json!({"type": "content_block_stop"}));
            data.push(json!({
                "type": "message_delta",
                "usage": {"output_tokens": 3},
                "delta": {"stop_reason": stop_reason}
            }));
            data.push(json!({"type": "message_stop"}));
            sse("\n", &data)
        }
        ModelApi::GenerateContent => {
            let usage = json!({"promptTokenCount": 12, "candidatesTokenCount": 4});
            let chunk = |part: Value, last: bool| {
                let mut candidate = json!({"content": {"role": "model", "parts": [part]}});
                if last {
                    candidate["finishReason"] = json!("STOP");
                }
                json!({"candidates": [candidate], "usageMetadata": usage})
            };
            let data: Vec<Value> = match reply {
                Reply::ToolCall => vec![chunk(
                    json!({"functionCall": {"name": TOOL, "args": {}}}),
                    true,
                )],
                Reply::Text | Reply::Json => parts
                    .iter()
                    .enumerate()
                    .map(|(index, part)| chunk(json!({"text": part}), index + 1 == parts.len()))
                    .collect(),
            };
            sse("\n", &data)
        }
        ModelApi::Realtime => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// Offers [`TOOL`] and records each call of it.
#[derive(Default)]
struct RecordingTools {
    dispatched: Mutex<Vec<String>>,
}

#[async_trait]
impl AgentToolDispatcher for RecordingTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::new([Arc::new(ToolDef {
            audience: Default::default(),
            name: TOOL.into(),
            description: "Look something up".to_string(),
            input_schema: json!({"type": "object", "properties": {}}),
            provenance: None,
        })])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatched.lock().unwrap().push(call.name.to_string());
        Ok(ToolDispatchOutcome::from(ToolResult::new(
            call.id.to_string(),
            "found".to_string(),
            false,
        )))
    }
}

struct NoTools;

#[async_trait]
impl AgentToolDispatcher for NoTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::from([])
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::not_found(call.name))
    }
}

// ---------------------------------------------------------------------------
// A seated Copilot account (hand-written list, loopback endpoint)
// ---------------------------------------------------------------------------

/// Signs like the Copilot authorizer and names the persistence the runtime
/// keys the account by.
struct SeatedAuthorizer {
    persistence_id: meerkat_core::auth::ProviderAuthPersistenceId,
}

#[async_trait]
impl meerkat_core::HttpAuthorizer for SeatedAuthorizer {
    async fn authorize(
        &self,
        request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
    ) -> Result<(), meerkat_core::AuthError> {
        request.headers.push((
            "Authorization".to_string(),
            format!("Bearer {}-{}", "stub", "token"),
        ));
        Ok(())
    }

    fn label(&self) -> &str {
        meerkat_copilot::GITHUB_COPILOT_AUTHORIZER_LABEL
    }

    fn persistence_authority_id(&self) -> Option<meerkat_core::auth::ProviderAuthPersistenceId> {
        Some(self.persistence_id)
    }
}

/// A Copilot runtime with one account whose model list the test writes. The
/// runtime resolves the list seated last; every seated state stays alive
/// with this value.
struct CopilotSeat {
    runtime: Arc<meerkat_copilot::CopilotRuntime>,
    persistence_id: meerkat_core::auth::ProviderAuthPersistenceId,
    account: meerkat_core::AuthCredentialIdentity,
    api_base: String,
    seated: Mutex<Vec<meerkat_copilot::CopilotSeatedAccount>>,
}

impl CopilotSeat {
    fn new(api_base: &str) -> Arc<Self> {
        let persistence_id = meerkat_core::auth::ProviderAuthPersistence::new(
            Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
            Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
        )
        .authority_id();
        let account =
            meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
                realm: meerkat_core::RealmId::global(),
                account: meerkat_core::CredentialAccountId::parse("github_copilot")
                    .expect("valid account"),
            });
        Arc::new(Self {
            runtime: Arc::new(meerkat_copilot::CopilotRuntime::new()),
            persistence_id,
            account,
            api_base: api_base.to_string(),
            seated: Mutex::new(Vec::new()),
        })
    }

    /// Give the account `entries` as its model list.
    fn seat(&self, entries: &[Value]) {
        let listed: meerkat_copilot::CopilotModelsEnvelope =
            serde_json::from_value(json!({"data": entries})).expect("the account's list");
        let seated = self
            .runtime
            .seat_account_for_tests(
                self.persistence_id,
                &self.account,
                &self.api_base,
                listed.data,
            )
            .expect("the account is seated");
        self.seated.lock().unwrap().push(seated);
    }

    /// The session's text client for `model` of `provider` on this account,
    /// built the way an agent build composes it.
    fn text_client(&self, provider: Provider, model: &str, config: &Config) -> Arc<dyn LlmClient> {
        use meerkat_llm_core::provider_runtime::binding::{
            DynamicLease, NormalizedBackendKind, ResolvedConnection, ResolvedTextTarget,
        };
        use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;

        let (binding, backend) = match provider {
            Provider::OpenAI => (
                "copilot_openai",
                NormalizedBackendKind::OpenAi(OpenAiBackendKind::Copilot),
            ),
            Provider::Anthropic => (
                "copilot_anthropic",
                NormalizedBackendKind::Anthropic(AnthropicBackendKind::Copilot),
            ),
            Provider::Gemini => (
                "copilot_gemini",
                NormalizedBackendKind::Google(GoogleBackendKind::Copilot),
            ),
            other => panic!("no Copilot backend for {other:?}"),
        };
        let identity = meerkat_core::SessionLlmIdentity {
            model: model.to_string(),
            provider,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::global(),
                binding: meerkat_core::BindingId::parse(binding).expect("valid binding"),
                profile: None,
                origin: meerkat_core::BindingOrigin::Configured,
            }),
        };
        let profile = meerkat_core::ModelRegistry::from_config(config, meerkat_models::canonical())
            .expect("registry")
            .profile_witness_for_provider(provider, model)
            .expect("the registry defines the model");
        let connection = ResolvedConnection {
            provider,
            backend,
            backend_profile: Arc::new(meerkat_core::BackendProfile {
                id: binding.to_string(),
                provider,
                backend_kind: "copilot".to_string(),
                base_url: None,
                options: Value::Null,
                server: None,
            }),
            credential_identity: self.account.clone(),
            auth_lease: Arc::new(DynamicLease::from_authorizer(
                Arc::new(SeatedAuthorizer {
                    persistence_id: self.persistence_id,
                }),
                meerkat_core::AuthMetadata::default(),
                meerkat_copilot::GITHUB_COPILOT_AUTHORIZER_LABEL,
            )),
        };
        let target = ResolvedTextTarget::new(identity, profile, connection)
            .expect("the target names one provider");
        let runtime = Arc::clone(&self.runtime);
        match provider {
            Provider::OpenAI => meerkat_openai::OpenAiProviderRuntime::with_copilot(runtime)
                .build_text_client(target),
            Provider::Anthropic => {
                meerkat_anthropic::AnthropicProviderRuntime::with_copilot(runtime)
                    .build_text_client(target)
            }
            _ => meerkat_gemini::GoogleProviderRuntime::with_copilot(
                runtime,
                Arc::new(meerkat_openai::OpenAiCopilotChatCompletionsClientFactory),
            )
            .build_text_client(target),
        }
        .unwrap_or_else(|error| panic!("the session's text client builds: {error}"))
    }
}

// ---------------------------------------------------------------------------
// The parameter table
// ---------------------------------------------------------------------------

/// One route a session can be served on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Param {
    OpenAiDirect,
    ClaudeDirect,
    GeminiDirect,
    SelfHostedChat,
    SelfHostedResponses,
    /// Copilot, OpenAI family, Responses: an id the catalog also has.
    CopilotResponses,
    /// Copilot, OpenAI family, Responses: an id the catalog does not have.
    CopilotResponsesUncatalogued,
    /// Copilot, OpenAI family, Chat Completions: an id of the account's own.
    CopilotChat,
    /// Copilot, Claude, Messages: the catalog id, listed with a dot.
    CopilotClaude,
    /// Copilot, Gemini, Chat Completions.
    CopilotGemini,
}

const PARAMS: [Param; 10] = [
    Param::OpenAiDirect,
    Param::ClaudeDirect,
    Param::GeminiDirect,
    Param::SelfHostedChat,
    Param::SelfHostedResponses,
    Param::CopilotResponses,
    Param::CopilotResponsesUncatalogued,
    Param::CopilotChat,
    Param::CopilotClaude,
    Param::CopilotGemini,
];

const COPILOT_PARAMS: [Param; 5] = [
    Param::CopilotResponses,
    Param::CopilotResponsesUncatalogued,
    Param::CopilotChat,
    Param::CopilotClaude,
    Param::CopilotGemini,
];

/// What the source of the route's facts says about the fixture's model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// The source states levels.
    Stated,
    /// No source states levels: a model without a catalog row on a direct
    /// route, no operator levels on a self-hosted route, an entry that
    /// states none (for a model the catalog has) on a Copilot route.
    Unknown,
    /// Copilot Messages only: the entry states levels and states that the
    /// model does not take adaptive thinking.
    NoAdaptiveThinking,
    /// Gemini direct only: a catalog row that records no levels.
    RowWithoutLevels,
}

const LADDER: [EffortLevel; 7] = [
    EffortLevel::None,
    EffortLevel::Minimal,
    EffortLevel::Low,
    EffortLevel::Medium,
    EffortLevel::High,
    EffortLevel::Xhigh,
    EffortLevel::Max,
];

/// A model id no catalog row and no account entry is written for.
const UNCATALOGUED: &str = "fixture-reasoner-1";
const SELF_HOSTED_MODEL: &str = "remote-model";
const OPERATOR_LEVELS: [EffortLevel; 2] = [EffortLevel::Low, EffortLevel::High];
/// What a Copilot entry of these fixtures states.
const ENTRY_LEVELS: [EffortLevel; 2] = [EffortLevel::Low, EffortLevel::Medium];

fn wire(levels: &[EffortLevel]) -> Vec<&'static str> {
    levels.iter().map(|level| level.as_wire_str()).collect()
}

/// What the catalog row of `model` states on the provider's direct route.
fn catalog_facts(provider: Provider, model: &str) -> ReasoningLevelFacts {
    meerkat_core::supported_reasoning_levels(
        provider,
        meerkat_core::direct_route_observation(provider).as_ref(),
        meerkat_models::canonical().capabilities_for(provider, model),
    )
}

/// The catalog models of `provider`, its default first.
fn catalog_models(provider: Provider) -> impl Iterator<Item = &'static str> {
    meerkat_models::default_model(provider)
        .into_iter()
        .chain(meerkat_models::allowed_models(provider))
}

/// A catalog model of `provider` whose row states some levels and not every
/// level, with those levels as the direct route can express them. `also`
/// narrows the choice.
fn catalog_model(provider: Provider, also: impl Fn(&str) -> bool) -> (String, Vec<EffortLevel>) {
    catalog_models(provider)
        .filter(|model| also(model))
        .find_map(|model| match catalog_facts(provider, model) {
            ReasoningLevelFacts::Stated { levels, .. }
                if levels.len() >= 2 && levels.len() < LADDER.len() =>
            {
                Some((model.to_string(), levels))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("the catalog has a {provider:?} model that states levels"))
}

/// A catalog id that ends in two short numbers, written with a dot between
/// them, the way an account can list it.
fn dotted(model: &str) -> Option<String> {
    let short_number = |part: &str| {
        !part.is_empty() && part.len() <= 2 && part.bytes().all(|b| b.is_ascii_digit())
    };
    let (head, minor) = model.rsplit_once('-')?;
    let (_, major) = head.rsplit_once('-')?;
    (short_number(major) && short_number(minor)).then(|| format!("{head}.{minor}"))
}

/// Whether the catalog row of the Anthropic `model` accepts the
/// `between_tools` thinking type beside `adaptive`.
fn between_tools_row(model: &str) -> bool {
    use meerkat_core::model_profile::capabilities::ThinkingSupport;
    meerkat_models::canonical()
        .capabilities_for(Provider::Anthropic, model)
        .is_some_and(|row| row.thinking == ThinkingSupport::AnthropicAdaptiveOrBetweenTools)
}

fn custom_model(config: &mut Config, id: &str, provider: Provider) {
    config.models.custom.insert(
        id.to_string(),
        meerkat_core::config::CustomModelConfig {
            provider,
            display_name: None,
            context_window: Some(200_000),
            max_input_tokens: None,
            max_output_tokens: Some(8_192),
            vision: None,
            web_search: None,
            call_timeout_secs: None,
        },
    );
}

fn stub_key() -> String {
    format!("{}-{}", "stub", "key")
}

/// One parameter of the table, staged: the session's model, the real client
/// of the route against the stub, and what the route's facts state.
struct Fixture {
    param: Param,
    provider: Provider,
    /// The model the session names.
    model: String,
    /// The model the provider request names.
    wire_model: String,
    route: ReasoningRoute,
    facts: ReasoningFactsSource,
    /// The levels the route accepts for the model, as it can express them.
    supported: Vec<EffortLevel>,
    /// The levels the catalog row states on the direct route, for a model
    /// the catalog has.
    catalog: Option<Vec<EffortLevel>>,
    client: Arc<dyn LlmClient>,
    config: Config,
    copilot: Option<Arc<CopilotSeat>>,
    /// The Copilot entry as seated.
    entry: Option<Value>,
}

impl Param {
    fn route(self) -> ReasoningRoute {
        let (backend, api) = match self {
            Self::OpenAiDirect => (
                ReasoningBackend::OpenAi(OpenAiBackendKind::OpenAiApi),
                ModelApi::Responses,
            ),
            Self::ClaudeDirect => (
                ReasoningBackend::Anthropic(AnthropicBackendKind::AnthropicApi),
                ModelApi::Messages,
            ),
            Self::GeminiDirect => (
                ReasoningBackend::Google(GoogleBackendKind::GoogleGenAi),
                ModelApi::GenerateContent,
            ),
            Self::SelfHostedChat => (
                ReasoningBackend::SelfHosted(SelfHostedBackendKind::SelfHosted),
                ModelApi::ChatCompletions,
            ),
            Self::SelfHostedResponses => (
                ReasoningBackend::SelfHosted(SelfHostedBackendKind::SelfHosted),
                ModelApi::Responses,
            ),
            Self::CopilotResponses | Self::CopilotResponsesUncatalogued => (
                ReasoningBackend::OpenAi(OpenAiBackendKind::Copilot),
                ModelApi::Responses,
            ),
            Self::CopilotChat => (
                ReasoningBackend::OpenAi(OpenAiBackendKind::Copilot),
                ModelApi::ChatCompletions,
            ),
            Self::CopilotClaude => (
                ReasoningBackend::Anthropic(AnthropicBackendKind::Copilot),
                ModelApi::Messages,
            ),
            Self::CopilotGemini => (
                ReasoningBackend::Google(GoogleBackendKind::Copilot),
                ModelApi::ChatCompletions,
            ),
        };
        ReasoningRoute::new(backend, api)
    }

    fn is_copilot(self) -> bool {
        COPILOT_PARAMS.contains(&self)
    }

    /// The wire paths the route writes a level to.
    fn fields(self) -> Vec<String> {
        let fields: &[&str] = match self {
            Self::OpenAiDirect
            | Self::SelfHostedResponses
            | Self::CopilotResponses
            | Self::CopilotResponsesUncatalogued => &["reasoning.effort"],
            Self::SelfHostedChat | Self::CopilotChat | Self::CopilotGemini => {
                &["reasoning_effort", "reasoning.effort"]
            }
            Self::ClaudeDirect => &["output_config.effort"],
            Self::CopilotClaude => &["thinking.type", "output_config.effort"],
            Self::GeminiDirect => &["generationConfig.thinkingConfig.thinkingLevel"],
        };
        fields.iter().map(|field| (*field).to_string()).collect()
    }

    /// The route's own effort field, as a request body carries `level`.
    fn assert_golden(self, body: &Value, level: EffortLevel) {
        let wire = level.as_wire_str();
        let context = format!("{self:?}: {body}");
        match self {
            Self::OpenAiDirect | Self::SelfHostedResponses | Self::CopilotResponses => {
                assert_eq!(body["reasoning"]["effort"], wire, "{context}");
            }
            Self::CopilotResponsesUncatalogued => {
                assert_eq!(body["reasoning"], json!({"effort": wire}), "{context}");
                assert!(body.get("include").is_none(), "{context}");
            }
            Self::SelfHostedChat | Self::CopilotChat | Self::CopilotGemini => {
                assert_eq!(body["reasoning_effort"], wire, "{context}");
                assert_eq!(body["reasoning"]["effort"], wire, "{context}");
            }
            Self::ClaudeDirect => {
                assert_eq!(body["output_config"]["effort"], wire, "{context}");
            }
            Self::CopilotClaude => {
                assert_eq!(body["thinking"], json!({"type": "adaptive"}), "{context}");
                assert_eq!(body["output_config"]["effort"], wire, "{context}");
            }
            Self::GeminiDirect => {
                assert_eq!(
                    body["generationConfig"]["thinkingConfig"]["thinkingLevel"], wire,
                    "{context}"
                );
            }
        }
    }

    /// Stage this parameter against the stub at `base`. `None` when the
    /// stage does not exist for the parameter.
    fn fixture(self, stage: Stage, base: &str) -> Option<Fixture> {
        let route = self.route();
        let mut config = Config::default();
        let known = stage == Stage::Stated;
        let direct = |provider: Provider,
                      config: &mut Config|
         -> Option<(String, Vec<EffortLevel>, Option<Vec<EffortLevel>>)> {
            match stage {
                Stage::Stated => {
                    let (model, levels) = catalog_model(provider, |_| true);
                    Some((model, levels.clone(), Some(levels)))
                }
                Stage::Unknown => {
                    custom_model(config, UNCATALOGUED, provider);
                    Some((UNCATALOGUED.to_string(), Vec::new(), None))
                }
                Stage::RowWithoutLevels => catalog_models(provider)
                    .find(|model| {
                        matches!(catalog_facts(provider, model), ReasoningLevelFacts::Unknown)
                    })
                    .map(|model| (model.to_string(), Vec::new(), None)),
                Stage::NoAdaptiveThinking => None,
            }
        };
        match self {
            Self::OpenAiDirect => {
                if stage == Stage::RowWithoutLevels {
                    return None;
                }
                let (model, supported, catalog) = direct(Provider::OpenAI, &mut config)?;
                Some(Fixture {
                    param: self,
                    provider: Provider::OpenAI,
                    wire_model: model.clone(),
                    model,
                    route,
                    facts: ReasoningFactsSource::Catalog,
                    supported,
                    catalog,
                    client: Arc::new(meerkat::OpenAiClient::new_with_base_url(
                        stub_key(),
                        base.to_string(),
                    )),
                    config,
                    copilot: None,
                    entry: None,
                })
            }
            Self::ClaudeDirect => {
                if stage == Stage::RowWithoutLevels {
                    return None;
                }
                let (model, supported, catalog) = direct(Provider::Anthropic, &mut config)?;
                Some(Fixture {
                    param: self,
                    provider: Provider::Anthropic,
                    wire_model: model.clone(),
                    model,
                    route,
                    facts: ReasoningFactsSource::Catalog,
                    supported,
                    catalog,
                    client: Arc::new(
                        meerkat::AnthropicClient::new(stub_key())
                            .expect("a client")
                            .with_base_url(base.to_string()),
                    ),
                    config,
                    copilot: None,
                    entry: None,
                })
            }
            Self::GeminiDirect => {
                let (model, supported, catalog) = direct(Provider::Gemini, &mut config)?;
                Some(Fixture {
                    param: self,
                    provider: Provider::Gemini,
                    wire_model: model.clone(),
                    model,
                    route,
                    facts: ReasoningFactsSource::Catalog,
                    supported,
                    catalog,
                    client: Arc::new(meerkat::GeminiClient::new_with_base_url(
                        stub_key(),
                        base.to_string(),
                    )),
                    config,
                    copilot: None,
                    entry: None,
                })
            }
            Self::SelfHostedChat | Self::SelfHostedResponses => {
                use meerkat_openai::client_compatible::OpenAiCompatibleMode;
                if !matches!(stage, Stage::Stated | Stage::Unknown) {
                    return None;
                }
                let mode = if self == Self::SelfHostedChat {
                    OpenAiCompatibleMode::ChatCompletions
                } else {
                    OpenAiCompatibleMode::Responses
                };
                let client = meerkat_openai::OpenAiCompatibleClient::new_with_options(
                    mode,
                    SELF_HOSTED_MODEL.to_string(),
                    format!("{base}/v1"),
                    None,
                    meerkat_openai::OpenAiCompatibleClientOptions {
                        supports_temperature: true,
                        supports_thinking: true,
                        supports_reasoning: true,
                        supports_image_tool_results: false,
                    },
                )
                .with_self_hosted_backend(SelfHostedBackendKind::SelfHosted)
                .with_operator_reasoning_effort_levels(known.then(|| OPERATOR_LEVELS.to_vec()));
                Some(Fixture {
                    param: self,
                    provider: Provider::SelfHosted,
                    model: SELF_HOSTED_MODEL.to_string(),
                    wire_model: SELF_HOSTED_MODEL.to_string(),
                    route,
                    facts: ReasoningFactsSource::Operator,
                    supported: if known {
                        OPERATOR_LEVELS.to_vec()
                    } else {
                        Vec::new()
                    },
                    catalog: None,
                    client: Arc::new(client),
                    config,
                    copilot: None,
                    entry: None,
                })
            }
            Self::CopilotResponses
            | Self::CopilotResponsesUncatalogued
            | Self::CopilotChat
            | Self::CopilotClaude
            | Self::CopilotGemini => {
                let provider = match self {
                    Self::CopilotClaude => Provider::Anthropic,
                    Self::CopilotGemini => Provider::Gemini,
                    _ => Provider::OpenAI,
                };
                match (stage, self) {
                    (Stage::Stated | Stage::Unknown, _)
                    | (Stage::NoAdaptiveThinking, Self::CopilotClaude) => {}
                    _ => return None,
                }
                // The session's model and the id the account lists it under.
                // An unknown-support stage always names a model the catalog
                // has: its row must not stand in for the entry's silence.
                let synthetic =
                    known && matches!(self, Self::CopilotResponsesUncatalogued | Self::CopilotChat);
                let (model, listed_id, catalog) = if synthetic {
                    custom_model(&mut config, UNCATALOGUED, provider);
                    (UNCATALOGUED.to_string(), UNCATALOGUED.to_string(), None)
                } else if provider == Provider::Anthropic {
                    // Prefer a model whose row accepts a second thinking
                    // type, so a session can set one.
                    let (model, levels) = catalog_models(provider)
                        .find(|model| dotted(model).is_some() && between_tools_row(model))
                        .map_or_else(
                            || catalog_model(provider, |model| dotted(model).is_some()),
                            |preferred| catalog_model(provider, |model| model == preferred),
                        );
                    let listed_id = dotted(&model).expect("a dotted spelling");
                    (model, listed_id, Some(levels))
                } else {
                    let (model, levels) = catalog_model(provider, |_| true);
                    (model.clone(), model, Some(levels))
                };
                if provider == Provider::Gemini {
                    // This build hands the factory a finished client, so
                    // the factory does not know the backend and would add
                    // the direct API's native search default, which the
                    // Chat Completions route refuses.
                    config.provider_tools.gemini.google_search = false;
                }
                let endpoint = match route.api {
                    ModelApi::Responses => "/responses",
                    ModelApi::Messages => "/v1/messages",
                    _ => "/chat/completions",
                };
                // The Gemini entry also states `minimal`, which the Chat
                // Completions effort field cannot express.
                let stated: Vec<&str> = if self == Self::CopilotGemini {
                    std::iter::once("minimal")
                        .chain(wire(&ENTRY_LEVELS))
                        .collect()
                } else {
                    wire(&ENTRY_LEVELS)
                };
                let mut supports = serde_json::Map::new();
                if stage != Stage::Unknown {
                    supports.insert("reasoning_effort".to_string(), json!(stated));
                }
                if self == Self::CopilotClaude {
                    supports.insert(
                        "adaptive_thinking".to_string(),
                        json!(stage != Stage::NoAdaptiveThinking),
                    );
                }
                let entry = json!({
                    "id": listed_id,
                    "supported_endpoints": [endpoint],
                    "capabilities": {"supports": supports}
                });
                let seat = CopilotSeat::new(base);
                seat.seat(std::slice::from_ref(&entry));
                let client = seat.text_client(provider, &model, &config);
                Some(Fixture {
                    param: self,
                    provider,
                    model,
                    wire_model: listed_id,
                    route,
                    facts: ReasoningFactsSource::Account,
                    supported: if known {
                        ENTRY_LEVELS.to_vec()
                    } else {
                        Vec::new()
                    },
                    catalog,
                    client,
                    config,
                    copilot: Some(seat),
                    entry: Some(entry),
                })
            }
        }
    }
}

impl Fixture {
    /// A level the route accepts for the model: an active one where the set
    /// has one.
    fn level(&self) -> EffortLevel {
        self.supported
            .iter()
            .copied()
            .find(|level| *level >= EffortLevel::Low)
            .or_else(|| self.supported.first().copied())
            .expect("the fixture states levels")
    }

    /// A level outside the fixture's set: the lowest active one where there
    /// is one.
    fn unsupported_level(&self) -> EffortLevel {
        let outside = |level: &EffortLevel| !self.supported.contains(level);
        LADDER
            .into_iter()
            .find(|level| *level >= EffortLevel::Low && outside(level))
            .or_else(|| LADDER.into_iter().find(outside))
            .expect("a level outside the set")
    }

    /// What a request sent with `level` reports.
    fn resolution(&self, level: EffortLevel) -> ReasoningEffortResolution {
        ReasoningEffortResolution {
            requested: level,
            level,
            provider: self.provider,
            model: self.model.clone(),
            route: self.route.clone(),
            fields: self.param.fields(),
            facts: self.facts,
            decided_by: ReasoningEffortDecidedBy::Request,
        }
    }

    /// Every recorded request went to the route's API, named the route's
    /// wire model and carried `level` in the route's field.
    fn assert_requests(&self, recorded: &[Recorded], level: EffortLevel) {
        for request in recorded {
            assert_eq!(
                api_of_path(&request.path),
                Some(self.route.api),
                "{:?}: {}",
                self.param,
                request.path
            );
            if self.route.api == ModelApi::GenerateContent {
                assert!(
                    request.path.contains(&self.wire_model),
                    "{:?}: {}",
                    self.param,
                    request.path
                );
            } else {
                assert_eq!(request.body["model"], self.wire_model, "{:?}", self.param);
            }
            self.param.assert_golden(&request.body, level);
        }
    }
}

// ---------------------------------------------------------------------------
// Building and running
// ---------------------------------------------------------------------------

fn level_params(level: EffortLevel) -> ProviderParamsOverride {
    ProviderParamsOverride {
        reasoning_effort: Some(level),
        ..Default::default()
    }
}

fn answer_schema() -> OutputSchema {
    OutputSchema::new(json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
        "additionalProperties": false
    }))
    .expect("a schema")
}

#[derive(Default)]
struct Build {
    params: Option<ProviderParamsOverride>,
    tools: Option<Arc<dyn AgentToolDispatcher>>,
    schema: Option<OutputSchema>,
    resume: Option<Session>,
    /// Stands between the adapter and the fixture's client.
    client: Option<Arc<dyn LlmClient>>,
    /// Receives the adapter's live stream events (text deltas).
    live: Option<tokio::sync::mpsc::Sender<AgentEvent>>,
}

impl Build {
    fn level(level: EffortLevel) -> Self {
        Self {
            params: Some(level_params(level)),
            ..Self::default()
        }
    }
}

async fn build(fixture: &Fixture, build: Build) -> Result<DynAgent, BuildAgentError> {
    let factory = AgentFactory::new(".rkat/sessions");
    let client = build.client.unwrap_or_else(|| Arc::clone(&fixture.client));
    let llm = Arc::new(
        factory
            .build_llm_adapter_with_events(client, fixture.model.clone(), build.live)
            .await,
    );
    let store = Arc::new(
        factory
            .build_store_adapter(Arc::new(TestSessionStore::new()))
            .await,
    );
    let tools = build.tools.unwrap_or_else(|| Arc::new(NoTools));
    let mut builder = AgentBuilder::new()
        .with_config(fixture.config.clone())
        .model(fixture.model.clone())
        .max_tokens_per_turn(256)
        .retry_policy(RetryPolicy {
            max_retries: 2,
            initial_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            ..RetryPolicy::default()
        });
    if let Some(params) = build.params {
        builder = builder.provider_params(params);
    }
    if let Some(schema) = build.schema {
        builder = builder.output_schema(schema);
    }
    if let Some(session) = build.resume {
        builder = builder.resume_session(session);
    }
    builder.build(llm, tools, store).await
}

async fn run(
    agent: &mut DynAgent,
    prompt: &str,
) -> (Result<meerkat::RunResult, AgentError>, Vec<AgentEvent>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4096);
    let result = agent.run_with_events(prompt.to_string().into(), tx).await;
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    (result, events)
}

/// The per-request reports of a run, in order.
fn request_reports(events: &[AgentEvent]) -> Vec<ReasoningEffortResolution> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RequestReasoningEffortResolved { resolution, .. } => {
                Some(resolution.clone())
            }
            _ => None,
        })
        .collect()
}

/// What `run_started` reported, once per `run_started` event.
fn run_started_reports(events: &[AgentEvent]) -> Vec<Option<ReasoningEffortResolution>> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RunStarted {
                reasoning_effort, ..
            } => Some(reasoning_effort.clone()),
            _ => None,
        })
        .collect()
}

fn event_types(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| {
            serde_json::to_value(event).expect("an event serializes")["type"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

fn stored_identity(agent: &DynAgent) -> Value {
    serde_json::to_value(agent.session().session_metadata()).expect("metadata serializes")
}

/// A run that must fail with the typed refusal: nothing is reported as
/// sent, no model fallback is tried, the failure names the refusal, and the
/// session's stored identity is as it was.
async fn refused_run(agent: &mut DynAgent, context: &str) -> ReasoningEffortRefusal {
    let before = stored_identity(agent);
    let (result, events) = run(agent, "Look it up.").await;
    let refusal = match result {
        Err(AgentError::ReasoningEffortRefused(refusal)) => *refusal,
        other => panic!("{context}: expected the typed refusal, got {other:?}"),
    };
    assert_eq!(
        stored_identity(agent),
        before,
        "{context}: the stored identity is unchanged"
    );
    assert!(
        request_reports(&events).is_empty(),
        "{context}: no request is reported as sent"
    );
    let types = event_types(&events);
    assert!(
        !types.iter().any(|name| name.starts_with("model_fallback")),
        "{context}: no model fallback: {types:?}"
    );
    for event in &events {
        if let AgentEvent::RunFailed { error_report, .. } = event {
            assert_eq!(
                error_report.reason,
                Some(AgentErrorReason::ReasoningEffortRefused {
                    refusal: Box::new(refusal.clone()),
                }),
                "{context}"
            );
        }
    }
    refusal
}

/// Build and run a session that must be refused, at the build or by the
/// run, and return the typed refusal.
async fn refused(fixture: &Fixture, params: ProviderParamsOverride) -> ReasoningEffortRefusal {
    let context = format!("{:?}", fixture.param);
    let built = build(
        fixture,
        Build {
            params: Some(params),
            ..Build::default()
        },
    )
    .await;
    match built {
        Err(BuildAgentError::ReasoningEffortRefused(refusal)) => *refusal,
        Err(other) => panic!("{context}: expected the typed refusal, got {other}"),
        Ok(mut agent) => refused_run(&mut agent, &context).await,
    }
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// A level the route's facts admit completes a run that calls a tool,
/// streams text and extracts structured output: every request carries the
/// level in the route's own field, and the run reports it at its start, per
/// request and in its result. A session that sets no level reports nothing.
#[tokio::test]
async fn supported_level_completes_with_tools_schema_and_streaming() {
    for param in PARAMS {
        let stub = Stub::spawn(&[Reply::ToolCall, Reply::Text, Reply::Json]).await;
        let fixture = param.fixture(Stage::Stated, &stub.base).expect("staged");
        let level = fixture.level();
        let expected = fixture.resolution(level);
        let tools = Arc::new(RecordingTools::default());
        let (live_tx, mut live_rx) = tokio::sync::mpsc::channel(4096);
        let mut agent = build(
            &fixture,
            Build {
                tools: Some(tools.clone()),
                schema: Some(answer_schema()),
                live: Some(live_tx),
                ..Build::level(level)
            },
        )
        .await
        .unwrap_or_else(|error| panic!("{param:?}: the agent builds: {error}"));

        let (result, events) = run(&mut agent, "Look it up.").await;
        let result = result.unwrap_or_else(|error| panic!("{param:?}: the run completes: {error}"));
        let recorded = stub.recorded();

        // A tool call turn, the answer turn, the structured-output request.
        assert_eq!(recorded.len(), 3, "{param:?}: {recorded:?}");
        fixture.assert_requests(&recorded, level);
        assert_eq!(
            *tools.dispatched.lock().unwrap(),
            [TOOL.to_string()],
            "{param:?}: the tool ran"
        );
        assert_eq!(
            result.structured_output,
            Some(json!({"answer": "done"})),
            "{param:?}"
        );
        let mut streamed = String::new();
        while let Ok(event) = live_rx.try_recv() {
            if let AgentEvent::TextDelta { delta, .. } = event {
                streamed.push_str(&delta);
            }
        }
        assert!(
            streamed.contains(&ANSWER.concat()),
            "{param:?}: the answer streamed: {streamed:?}"
        );

        let report = result
            .reasoning_effort
            .unwrap_or_else(|| panic!("{param:?}: the result reports the level"));
        assert_eq!(report.requested, level, "{param:?}");
        assert_eq!(report.sent.len(), 1, "{param:?}: {report:?}");
        let sent = &report.sent[0];
        assert_eq!(
            (
                sent.level,
                sent.provider,
                sent.model.as_str(),
                &sent.route,
                &sent.fields,
                sent.facts,
                sent.decided_by,
            ),
            (
                level,
                fixture.provider,
                fixture.model.as_str(),
                &fixture.route,
                &fixture.param.fields(),
                fixture.facts,
                ReasoningEffortDecidedBy::Request,
            ),
            "{param:?}"
        );
        assert_eq!(sent.requests as usize, recorded.len(), "{param:?}");
        assert_eq!(
            request_reports(&events),
            vec![expected.clone(); recorded.len()],
            "{param:?}: one report per request"
        );
        assert_eq!(
            run_started_reports(&events),
            [Some(expected)],
            "{param:?}: the run start reports the same resolution"
        );

        // The same session without the level: one request, nothing reported.
        let control = Stub::spawn(&[Reply::Text]).await;
        let fixture = param.fixture(Stage::Stated, &control.base).expect("staged");
        let mut agent = build(&fixture, Build::default())
            .await
            .unwrap_or_else(|error| panic!("{param:?}: the control builds: {error}"));
        let (result, events) = run(&mut agent, "Hello.").await;
        let result = result.unwrap_or_else(|error| panic!("{param:?}: the control runs: {error}"));
        assert_eq!(control.recorded().len(), 1, "{param:?}");
        assert_eq!(result.reasoning_effort, None, "{param:?}");
        assert!(request_reports(&events).is_empty(), "{param:?}");
        assert_eq!(run_started_reports(&events), [None], "{param:?}");
    }
}

/// A level the route's facts do not admit is refused with the typed refusal,
/// which lists the levels the route accepts and their source, before any
/// request is sent.
#[tokio::test]
async fn unsupported_level_is_refused_before_any_provider_request() {
    for param in PARAMS {
        let stub = Stub::spawn(&[Reply::Text]).await;
        let fixture = param.fixture(Stage::Stated, &stub.base).expect("staged");

        let mut levels = vec![fixture.unsupported_level()];
        // Claude takes no `none`.
        if fixture.provider == Provider::Anthropic {
            levels.push(EffortLevel::None);
        }
        // The Chat Completions effort field has no `minimal`, though the
        // Gemini entry states it.
        if param == Param::CopilotGemini {
            levels.push(EffortLevel::Minimal);
        }
        // On Copilot a level the catalog row lists and the entry does not:
        // the row is not a source there.
        if param.is_copilot()
            && let Some(level) = fixture
                .catalog
                .as_ref()
                .and_then(|row| row.iter().find(|level| !fixture.supported.contains(level)))
        {
            levels.push(*level);
        }
        levels.dedup();

        for level in levels {
            let refusal = refused(&fixture, level_params(level)).await;
            assert_eq!(
                refusal,
                ReasoningEffortRefusal {
                    requested: level,
                    provider: fixture.provider,
                    model: fixture.model.clone(),
                    route: Some(fixture.route.clone()),
                    reason: ReasoningEffortRefusalReason::UnsupportedLevel,
                    supported: Some(fixture.supported.clone()),
                    facts: Some(fixture.facts),
                    conflicting_param: None,
                    conflicting_level: None,
                },
                "{param:?} at {level:?}"
            );
        }

        // Copilot Messages takes a level only with adaptive thinking: any
        // other thinking type the session sets conflicts, at a level and
        // on a model that accept that type on the direct API.
        if param == Param::CopilotClaude {
            assert!(
                between_tools_row(&fixture.model),
                "the catalog has a model that accepts a second thinking type"
            );
            let level = fixture.level();
            assert!(level <= EffortLevel::High);
            let refusal = refused(
                &fixture,
                ProviderParamsOverride {
                    provider_tag: Some(ProviderTag::Anthropic(AnthropicProviderTag {
                        thinking: Some(AnthropicThinkingConfig::BetweenTools),
                        ..Default::default()
                    })),
                    ..level_params(level)
                },
            )
            .await;
            assert_eq!(
                (
                    refusal.reason,
                    refusal.requested,
                    refusal.route.as_ref(),
                    refusal.supported.as_deref(),
                    refusal.facts,
                ),
                (
                    ReasoningEffortRefusalReason::ConflictingThinkingType,
                    level,
                    Some(&fixture.route),
                    Some(fixture.supported.as_slice()),
                    Some(fixture.facts),
                ),
                "{param:?}"
            );
            assert!(refusal.conflicting_param.is_some(), "{param:?}");
        }

        assert_eq!(
            stub.recorded().len(),
            0,
            "{param:?}: nothing is sent: {:?}",
            stub.recorded()
        );
    }
}

/// A level nothing states the facts for is refused as unknown before any
/// request is sent: a model without a catalog row, a row that records no
/// levels, a self-hosted model without operator levels, a Copilot entry
/// that states none (whatever the catalog row of the same model lists), and
/// a Copilot Messages entry that refuses adaptive thinking.
#[tokio::test]
async fn unknown_support_is_refused_before_any_provider_request() {
    let mut cases = Vec::new();
    for param in PARAMS {
        cases.push((param, Stage::Unknown));
    }
    cases.push((Param::CopilotClaude, Stage::NoAdaptiveThinking));
    // Staged only when the catalog has a Gemini row that records no levels.
    cases.push((Param::GeminiDirect, Stage::RowWithoutLevels));

    for (param, stage) in cases {
        let stub = Stub::spawn(&[Reply::Text]).await;
        let Some(fixture) = param.fixture(stage, &stub.base) else {
            assert_eq!(
                stage,
                Stage::RowWithoutLevels,
                "{param:?}: every other stage exists"
            );
            continue;
        };
        if param.is_copilot() {
            assert!(
                fixture.catalog.is_some(),
                "{param:?}: the catalog has a row for the model, and it is not used"
            );
        }
        let level = EffortLevel::Low;
        let refusal = refused(&fixture, level_params(level)).await;
        assert_eq!(
            refusal,
            ReasoningEffortRefusal {
                requested: level,
                provider: fixture.provider,
                model: fixture.model.clone(),
                route: Some(fixture.route.clone()),
                reason: ReasoningEffortRefusalReason::SupportedLevelsUnknown,
                supported: None,
                facts: None,
                conflicting_param: None,
                conflicting_level: None,
            },
            "{param:?} ({stage:?})"
        );
        assert_eq!(
            stub.recorded().len(),
            0,
            "{param:?} ({stage:?}): nothing is sent"
        );
    }
}

// ---------------------------------------------------------------------------
// A route that changes between a level's resolution and its request
// ---------------------------------------------------------------------------

/// Forwards everything to the route's client and, right after it answered
/// the `at_read`-th route read since it was armed, runs `change`: the
/// account's list changes between the agent's read of the route and the
/// client's preparation of the request.
struct ChangingRoute {
    inner: Arc<dyn LlmClient>,
    armed: AtomicBool,
    reads: AtomicUsize,
    at_read: usize,
    answers: Mutex<Vec<Option<ReasoningRouteObservation>>>,
    change: Box<dyn Fn() + Send + Sync>,
}

impl ChangingRoute {
    fn new(
        inner: Arc<dyn LlmClient>,
        at_read: usize,
        change: impl Fn() + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            armed: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
            at_read,
            answers: Mutex::new(Vec::new()),
            change: Box::new(change),
        })
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    /// The APIs the route reads since arming answered with, in order.
    fn answered_apis(&self) -> Vec<Option<ModelApi>> {
        self.answers
            .lock()
            .unwrap()
            .iter()
            .map(|answer| answer.as_ref().map(|observation| observation.api))
            .collect()
    }
}

#[async_trait]
impl LlmClient for ChangingRoute {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }

    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(logical_model)
    }

    fn reasoning_route(&self) -> Option<ReasoningRouteObservation> {
        let answer = self.inner.reasoning_route();
        if self.armed.load(Ordering::SeqCst) {
            self.answers.lock().unwrap().push(answer.clone());
            if self.reads.fetch_add(1, Ordering::SeqCst) + 1 == self.at_read {
                (self.change)();
            }
        }
        answer
    }

    fn reasoning_route_of(
        &self,
        projection: &LlmReplayProjection,
    ) -> Option<ReasoningRouteObservation> {
        self.inner.reasoning_route_of(projection)
    }

    fn project_replay_request(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<LlmReplayProjection, LlmError> {
        self.inner.project_replay_request(messages)
    }

    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn output_token_limit(&self) -> Option<u32> {
        self.inner.output_token_limit()
    }

    fn prompt_token_limit(&self, output_budget: u32) -> Option<u32> {
        self.inner.prompt_token_limit(output_budget)
    }

    fn request_pressure(
        &self,
        request: &LlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, LlmError> {
        self.inner.request_pressure(request)
    }

    fn prepared_request_pressure(
        &self,
        request: &PreparedLlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, LlmError> {
        self.inner.prepared_request_pressure(request)
    }

    fn authored_cache_breakpoints(
        &self,
        request: &LlmRequest,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, LlmError> {
        self.inner.authored_cache_breakpoints(request, messages)
    }

    fn prepared_cache_breakpoints(
        &self,
        request: &PreparedLlmRequest,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, LlmError> {
        self.inner.prepared_cache_breakpoints(request, messages)
    }

    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.inner.stream(request)
    }

    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        self.inner.stream_prepared(request)
    }

    fn provider(&self) -> Provider {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }

    fn compile_schema(
        &self,
        output_schema: &OutputSchema,
    ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::schema::SchemaError> {
        self.inner.compile_schema(output_schema)
    }
}

/// The second route read of a run is the read of the first request's
/// resolution: the first is the run-start report's.
const READ_OF_THE_FIRST_REQUEST: usize = 2;

/// The fixture's Copilot entry with `change` applied, seated when the
/// agent has read the route for its first request.
fn changing_route(fixture: &Fixture, change: impl Fn(&mut Value)) -> Arc<ChangingRoute> {
    let seat = Arc::clone(fixture.copilot.as_ref().expect("a Copilot fixture"));
    let mut entry = fixture.entry.clone().expect("the seated entry");
    change(&mut entry);
    ChangingRoute::new(
        Arc::clone(&fixture.client),
        READ_OF_THE_FIRST_REQUEST,
        move || seat.seat(std::slice::from_ref(&entry)),
    )
}

/// The account's list moves the model from Responses to Chat Completions
/// after the agent resolved the level for Responses and before the client
/// prepared the request: the level is resolved again for the route as it is
/// now, and the one request that is sent carries the Chat Completions
/// fields on the Chat Completions endpoint.
#[tokio::test]
async fn copilot_route_change_re_resolves_before_sending() {
    let stub = Stub::spawn(&[Reply::Text]).await;
    let fixture = Param::CopilotResponses
        .fixture(Stage::Stated, &stub.base)
        .expect("staged");
    let level = fixture.level();
    let client = changing_route(&fixture, |entry| {
        entry["supported_endpoints"] = json!(["/chat/completions"]);
    });
    let mut agent = build(
        &fixture,
        Build {
            client: Some(client.clone()),
            ..Build::level(level)
        },
    )
    .await
    .unwrap_or_else(|error| panic!("the agent builds: {error}"));

    client.arm();
    let (result, events) = run(&mut agent, "Hello.").await;
    let result = result.unwrap_or_else(|error| panic!("the run completes: {error}"));

    // The run start and the first resolution read Responses; the
    // preparation that was sent read Chat Completions.
    assert_eq!(
        client.answered_apis(),
        [
            Some(ModelApi::Responses),
            Some(ModelApi::Responses),
            Some(ModelApi::ChatCompletions),
        ]
    );
    let recorded = stub.recorded();
    assert_eq!(recorded.len(), 1, "exactly one request: {recorded:?}");
    assert!(
        recorded[0].path.ends_with("/chat/completions"),
        "{}",
        recorded[0].path
    );
    Param::CopilotChat.assert_golden(&recorded[0].body, level);

    let moved = ReasoningEffortResolution {
        route: Param::CopilotChat.route(),
        fields: Param::CopilotChat.fields(),
        ..fixture.resolution(level)
    };
    assert_eq!(moved.route.to_string(), "copilot/chat_completions");
    assert_eq!(request_reports(&events), [moved.clone()]);
    let report = result
        .reasoning_effort
        .expect("the result reports the level");
    assert_eq!(report.sent.len(), 1, "{report:?}");
    assert_eq!(
        (
            &report.sent[0].route,
            &report.sent[0].fields,
            report.sent[0].requests
        ),
        (&moved.route, &moved.fields, 1)
    );
}

/// The account's list changes only the levels the entry states, on the same
/// endpoint, after the agent resolved the level: the level is resolved
/// again against the new statement, which no longer holds it, and the turn
/// fails with the typed refusal that lists the new set. Nothing is sent.
#[tokio::test]
async fn a_refresh_that_changes_only_the_levels_re_resolves() {
    let stub = Stub::spawn(&[Reply::Text]).await;
    let fixture = Param::CopilotResponses
        .fixture(Stage::Stated, &stub.base)
        .expect("staged");
    let level = fixture.level();
    let remaining: Vec<EffortLevel> = fixture
        .supported
        .iter()
        .copied()
        .filter(|stated| *stated != level)
        .collect();
    assert!(!remaining.is_empty(), "the entry keeps a level");
    let stated = wire(&remaining);
    let client = changing_route(&fixture, |entry| {
        entry["capabilities"]["supports"]["reasoning_effort"] = json!(stated);
    });
    let mut agent = build(
        &fixture,
        Build {
            client: Some(client.clone()),
            ..Build::level(level)
        },
    )
    .await
    .unwrap_or_else(|error| panic!("the agent builds: {error}"));

    client.arm();
    let refusal = refused_run(&mut agent, "levels changed").await;
    assert_eq!(
        refusal,
        ReasoningEffortRefusal {
            requested: level,
            provider: fixture.provider,
            model: fixture.model.clone(),
            route: Some(fixture.route.clone()),
            reason: ReasoningEffortRefusalReason::UnsupportedLevel,
            supported: Some(remaining),
            facts: Some(ReasoningFactsSource::Account),
            conflicting_param: None,
            conflicting_level: None,
        }
    );
    assert_eq!(
        client.answered_apis(),
        [Some(ModelApi::Responses); 3],
        "the endpoint did not change"
    );
    assert_eq!(stub.recorded().len(), 0, "nothing is sent");
}
