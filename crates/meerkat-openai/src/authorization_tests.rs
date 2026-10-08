//! Actual registry companion forwarding and final OpenAI HTTP boundaries.
//!
//! Compatible routes use a fixture runtime only to select the real compatible
//! adapter. They do not claim coverage of the self-hosted credential resolver.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, Uri},
    response::IntoResponse,
};
use futures::StreamExt;
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse, OperationObservation,
    OperationObservationError, OperationObservedOutcome, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, PreparedOperationAuthorization, WorkAuthorization,
    WorkAuthorizationContext,
};
use meerkat_core::{
    AssistantBlock, AuthBindingRef, AuthCredentialIdentity, AuthMetadata, BackendProfile,
    BindingId, BindingOrigin, BlockAssistantMessage, Config, HttpAuthorizer,
    LlmRequestAuthorization, Message, ModelRegistry, OperationId, Provider, ProviderMeta, RealmId,
    ServerToolKind, SessionLlmIdentity, StopReason, UserMessage,
};
use meerkat_llm_core::provider_runtime::binding::{
    DynamicLease, NormalizedBackendKind, ResolvedConnection, ResolvedTextTarget, ValidatedBinding,
};
use meerkat_llm_core::provider_runtime::errors::{ProviderAuthError, ProviderClientError};
use meerkat_llm_core::provider_runtime::registry::{ProviderRuntimeRegistry, ResolverEnvironment};
use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, PreparedLlmRequest,
};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::client_compatible::{OpenAiCompatibleClient, OpenAiCompatibleMode};
use crate::{OpenAiBackendKind, OpenAiProviderRuntime};

const MODEL: &str = "gpt-5.4";
const REMOTE_MODEL: &str = "fixture-remote-model";
const RESPONSES_SSE: &str = concat!(
    "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n",
    "data: {\"type\":\"response.done\",\"response\":{\"id\":\"resp_test\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n",
    "data: [DONE]\n\n",
);
const CHAT_SSE: &str = concat!(
    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n",
    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n",
    "data: [DONE]\n\n",
);

#[derive(Clone, Copy, Debug)]
enum Route {
    Public,
    ChatGpt,
    Azure,
    CompatibleResponses,
    CompatibleChat,
}
const ROUTES: [Route; 5] = [
    Route::Public,
    Route::ChatGpt,
    Route::Azure,
    Route::CompatibleResponses,
    Route::CompatibleChat,
];
impl Route {
    fn path(self) -> &'static str {
        match self {
            Self::Public | Self::CompatibleResponses => "/v1/responses",
            Self::ChatGpt => "/responses",
            Self::Azure => "/openai/v1/responses",
            Self::CompatibleChat => "/v1/chat/completions",
        }
    }
    fn backend(self) -> OpenAiBackendKind {
        match self {
            Self::ChatGpt => OpenAiBackendKind::ChatGptBackend,
            Self::Azure => OpenAiBackendKind::AzureOpenAi,
            _ => OpenAiBackendKind::OpenAiApi,
        }
    }
    fn wire_model(self) -> &'static str {
        match self {
            Self::CompatibleResponses | Self::CompatibleChat => REMOTE_MODEL,
            _ => MODEL,
        }
    }
}

struct Policy {
    observations: Arc<Mutex<Vec<OperationObservation>>>,
    fail_entry: Arc<AtomicBool>,
    fail_outcome: Arc<AtomicBool>,
    allowed: Arc<AtomicBool>,
    bindings: Mutex<Vec<PreparedAuthorizationBinding>>,
    refresh_on_entry: bool,
    refreshed_dropped: Arc<AtomicBool>,
}
impl Policy {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            observations: Arc::new(Mutex::new(Vec::new())),
            fail_entry: Arc::new(AtomicBool::new(false)),
            fail_outcome: Arc::new(AtomicBool::new(false)),
            allowed: Arc::new(AtomicBool::new(true)),
            bindings: Mutex::new(Vec::new()),
            refresh_on_entry: false,
            refreshed_dropped: Arc::new(AtomicBool::new(false)),
        })
    }
    fn facts(&self) -> Vec<ModelAuthorizationFacts> {
        self.bindings
            .lock()
            .unwrap()
            .iter()
            .filter_map(|binding| {
                if let AuthorizationOperation::Model(facts) = &binding.facts().operation {
                    Some(facts.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}
struct Check {
    observations: Arc<Mutex<Vec<OperationObservation>>>,
    fail_entry: Arc<AtomicBool>,
    fail_outcome: Arc<AtomicBool>,
    allowed: Arc<AtomicBool>,
    binding: PreparedAuthorizationBinding,
    stale: bool,
    drop_flag: Option<Arc<AtomicBool>>,
}
impl Drop for Check {
    fn drop(&mut self) {
        if let Some(flag) = &self.drop_flag {
            flag.store(true, Ordering::SeqCst);
        }
    }
}
impl PreparedOperationAuthorization for Check {
    fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
        meerkat_core::authorization::OperationReviewTier::R1
    }

    fn observe(
        &self,
        binding: &PreparedAuthorizationBinding,
        observation: OperationObservation,
    ) -> Result<(), OperationObservationError> {
        if !self.binding.same_operation(binding) {
            return Err(OperationObservationError);
        }
        let fail = match &observation {
            OperationObservation::Entry => self.fail_entry.load(Ordering::SeqCst),
            OperationObservation::Outcome(_) => self.fail_outcome.load(Ordering::SeqCst),
            OperationObservation::ReviewAttemptStarted { .. } => false,
            OperationObservation::AuthorizationUnavailable => false,
            OperationObservation::Refused(_) => false,
        };
        self.observations.lock().unwrap().push(observation);
        if fail {
            Err(OperationObservationError)
        } else {
            Ok(())
        }
    }

    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        if !self.binding.same_operation(binding) || !self.allowed.load(Ordering::SeqCst) {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        if self.stale {
            return Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into());
        }
        Ok(())
    }
}
impl WorkAuthorization for Policy {
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, meerkat_core::OperationAuthorizationError>
    {
        if !matches!(binding.facts().operation, AuthorizationOperation::Model(_)) {
            return Err(OperationRefused::new(OperationRefusalKind::MalformedFacts).into());
        }
        let preparation = {
            let mut bindings = self.bindings.lock().unwrap();
            bindings.push(binding.clone());
            bindings.len()
        };
        if !self.allowed.load(Ordering::SeqCst) {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        Ok(Arc::new(Check {
            observations: Arc::clone(&self.observations),
            fail_entry: Arc::clone(&self.fail_entry),
            fail_outcome: Arc::clone(&self.fail_outcome),
            allowed: Arc::clone(&self.allowed),
            binding: binding.clone(),
            stale: self.refresh_on_entry && preparation == 1,
            drop_flag: (self.refresh_on_entry && preparation > 1)
                .then(|| Arc::clone(&self.refreshed_dropped)),
        }))
    }
}

struct Authorizer {
    entered: Notify,
    resume: Notify,
    pause: bool,
    revoke_on_response: Option<Arc<AtomicBool>>,
    calls: AtomicUsize,
}
impl Authorizer {
    fn new(pause: bool, revoke_on_response: Option<Arc<AtomicBool>>) -> Arc<Self> {
        Arc::new(Self {
            entered: Notify::new(),
            resume: Notify::new(),
            pause,
            revoke_on_response,
            calls: AtomicUsize::new(0),
        })
    }
}
#[async_trait]
impl HttpAuthorizer for Authorizer {
    async fn authorize(
        &self,
        request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
    ) -> Result<(), meerkat_core::AuthError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.pause {
            self.resume.notified().await;
        }
        request.headers.push((
            "Authorization".to_owned(),
            "Bearer synthetic-test-credential".to_owned(),
        ));
        Ok(())
    }
    async fn observe_response(
        &self,
        response: &meerkat_core::HttpAuthorizationResponse<'_>,
    ) -> Result<meerkat_core::HttpAuthorizationResponseAction, meerkat_core::AuthError> {
        if response.status == 401
            && let Some(allowed) = &self.revoke_on_response
        {
            allowed.store(false, Ordering::SeqCst);
            return Ok(meerkat_core::HttpAuthorizationResponseAction::RetryWithFreshAuthorization);
        }
        Ok(meerkat_core::HttpAuthorizationResponseAction::Propagate)
    }
    fn label(&self) -> &'static str {
        "openai-authorization-boundary-test"
    }
}

#[derive(Clone)]
struct ServerState {
    bodies: Arc<Mutex<Vec<(String, Value)>>>,
    status: StatusCode,
    redirect: Option<String>,
    reject_continuation: bool,
    revoke_continuation: Option<Arc<AtomicBool>>,
}
struct Server {
    url: String,
    bodies: Arc<Mutex<Vec<(String, Value)>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(
    status: StatusCode,
    redirect: Option<String>,
    reject_continuation: bool,
    revoke_continuation: Option<Arc<AtomicBool>>,
) -> Server {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let state = ServerState {
        bodies: Arc::clone(&bodies),
        status,
        redirect,
        reject_continuation,
        revoke_continuation,
    };
    let app = Router::new()
        .fallback(
            |State(state): State<ServerState>, uri: Uri, Json(body): Json<Value>| async move {
                let continuation = body.get("previous_response_id").is_some();
                state
                    .bodies
                    .lock()
                    .unwrap()
                    .push((uri.path().to_owned(), body));
                if continuation && state.reject_continuation {
                    if let Some(allowed) = state.revoke_continuation {
                        allowed.store(false, Ordering::SeqCst);
                    }
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error":{"message":"unknown previous_response_id"}})),
                    )
                        .into_response();
                }
                let payload = if uri.path().ends_with("chat/completions") {
                    CHAT_SSE
                } else {
                    RESPONSES_SSE
                };
                let mut response = (
                    state.status,
                    [("content-type", "text/event-stream")],
                    payload,
                )
                    .into_response();
                if let Some(location) = state.redirect {
                    response
                        .headers_mut()
                        .insert("location", location.parse().unwrap());
                }
                response
            },
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server { url, bodies, task }
}

// Select the real compatible adapter through the actual registry wrapper. This
// factory has no policy, grant, authentication or account-mapping semantics.
struct CompatibleFixtureRuntime {
    mode: OpenAiCompatibleMode,
}
#[async_trait]
impl ProviderRuntime for CompatibleFixtureRuntime {
    fn provider_id(&self) -> Provider {
        Provider::OpenAI
    }
    async fn resolve_binding(
        &self,
        _binding: &ValidatedBinding,
        _env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        Err(ProviderAuthError::Auth(
            meerkat_core::AuthError::MissingSecret,
        ))
    }
    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        let authorizer = connection
            .resolved_authorizer()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        let base = connection.backend_profile.base_url.as_deref().unwrap();
        Ok(Arc::new(
            OpenAiCompatibleClient::new(
                self.mode,
                REMOTE_MODEL.to_owned(),
                format!("{base}/v1"),
                None,
                true,
                false,
                false,
            )
            .with_authorizer(authorizer)
            .with_provider(Provider::OpenAI),
        ))
    }
}
fn binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse("test-realm").unwrap(),
        binding: BindingId::parse("test-binding").unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}
fn client(route: Route, url: &str, authorizer: Arc<Authorizer>) -> Arc<dyn LlmClient> {
    client_for_binding(route, url, authorizer, binding())
}
fn client_for_binding(
    route: Route,
    url: &str,
    authorizer: Arc<Authorizer>,
    binding: AuthBindingRef,
) -> Arc<dyn LlmClient> {
    let identity = SessionLlmIdentity {
        model: MODEL.to_owned(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(binding.clone()),
    };
    let models =
        ModelRegistry::from_config(&Config::default(), meerkat_models::canonical()).unwrap();
    let profile = models
        .profile_witness_for_provider(Provider::OpenAI, MODEL)
        .unwrap();
    let connection = ResolvedConnection {
        provider: Provider::OpenAI,
        backend: NormalizedBackendKind::OpenAi(route.backend()),
        backend_profile: Arc::new(BackendProfile {
            id: "test-backend".to_owned(),
            provider: Provider::OpenAI,
            backend_kind: route.backend().as_str().to_owned(),
            base_url: Some(url.to_owned()),
            options: Value::Null,
            server: None,
        }),
        credential_identity: AuthCredentialIdentity::Binding(binding),
        auth_lease: Arc::new(DynamicLease::from_authorizer(
            authorizer,
            AuthMetadata::default(),
            "synthetic",
        )),
    };
    let runtime: Arc<dyn ProviderRuntime> = match route {
        Route::CompatibleResponses => Arc::new(CompatibleFixtureRuntime {
            mode: OpenAiCompatibleMode::Responses,
        }),
        Route::CompatibleChat => Arc::new(CompatibleFixtureRuntime {
            mode: OpenAiCompatibleMode::ChatCompletions,
        }),
        _ => Arc::new(OpenAiProviderRuntime),
    };
    ProviderRuntimeRegistry::empty()
        .with_runtime(runtime)
        .build_text_client(ResolvedTextTarget::new(identity, profile, connection).unwrap())
        .unwrap()
}
fn request(client: &dyn LlmClient, policy: Arc<Policy>, request: LlmRequest) -> PreparedLlmRequest {
    let projection = client.project_replay_request(&request.messages).unwrap();
    let work = WorkAuthorizationContext::new(
        policy,
        meerkat_core::exact_operation::OperationExecutionScope::Domain,
    );
    PreparedLlmRequest::from_projection(request, projection).with_authorization(Some(
        LlmRequestAuthorization::new(work, OperationId::new(), ModelAuthorizationUse::Inference),
    ))
}
fn simple_request() -> LlmRequest {
    LlmRequest::new(MODEL, vec![Message::User(UserMessage::text("hello"))])
}
async fn collect(
    client: &dyn LlmClient,
    request: &PreparedLlmRequest,
) -> Vec<Result<LlmEvent, LlmError>> {
    tokio::time::timeout(
        Duration::from_secs(5),
        client.stream_prepared(request).collect(),
    )
    .await
    .unwrap()
}
fn stream_error(result: &Result<LlmEvent, LlmError>) -> Option<&LlmError> {
    match result {
        Err(error)
        | Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Error { error },
        }) => Some(error),
        Ok(_) => None,
    }
}

fn succeeded(results: &[Result<LlmEvent, LlmError>]) -> bool {
    results.iter().all(|result| stream_error(result).is_none())
        && matches!(
            results.last(),
            Some(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
            }))
        )
}

fn refused(results: &[Result<LlmEvent, LlmError>]) -> bool {
    results
        .iter()
        .filter_map(stream_error)
        .any(|error| matches!(error, LlmError::OperationRefused { .. }))
}

#[tokio::test]
async fn selected_target_and_actual_lowered_wire_model_reach_all_five_http_paths() {
    for route in ROUTES {
        let server = serve(StatusCode::OK, None, false, None).await;
        let policy = Policy::new();
        let client = client(route, &server.url, Authorizer::new(false, None));
        let request = request(client.as_ref(), Arc::clone(&policy), simple_request());
        let events = collect(client.as_ref(), &request).await;
        assert!(succeeded(&events), "{route:?}: {events:?}");
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Ok(LlmEvent::Done { .. })))
        );
        let bodies = server.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1, "{route:?}");
        assert_eq!(bodies[0].0, route.path());
        assert_eq!(bodies[0].1["model"], route.wire_model());
        let facts = policy.facts();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].identity.model, MODEL);
        assert_eq!(facts[0].wire_model.as_ref(), route.wire_model());
        assert_eq!(
            facts[0].endpoint.as_ref(),
            format!("{}{}", server.url, route.path())
        );
        assert_eq!(facts[0].backend_kind.as_ref(), route.backend().as_str());
        assert_eq!(
            facts[0].credential,
            Some(AuthCredentialIdentity::Binding(binding()))
        );
    }
}

#[tokio::test]
async fn revocation_while_credentials_are_awaited_prevents_every_http_body() {
    for route in ROUTES {
        let server = serve(StatusCode::OK, None, false, None).await;
        let policy = Policy::new();
        let authorizer = Authorizer::new(true, None);
        let client = client(route, &server.url, Arc::clone(&authorizer));
        let request = request(client.as_ref(), Arc::clone(&policy), simple_request());
        let task = tokio::spawn(async move { collect(client.as_ref(), &request).await });
        tokio::time::timeout(Duration::from_secs(5), authorizer.entered.notified())
            .await
            .unwrap();
        policy.allowed.store(false, Ordering::SeqCst);
        authorizer.resume.notify_one();
        assert!(refused(&task.await.unwrap()), "{route:?}");
        assert!(server.bodies.lock().unwrap().is_empty());
        assert_eq!(policy.facts().len(), 1);
    }
}

#[tokio::test]
async fn governed_http_never_follows_redirects_for_any_text_route() {
    for route in ROUTES {
        let destination = serve(StatusCode::OK, None, false, None).await;
        let server = serve(
            StatusCode::TEMPORARY_REDIRECT,
            Some(format!("{}{}", destination.url, route.path())),
            false,
            None,
        )
        .await;
        let client = client(route, &server.url, Authorizer::new(false, None));
        let request = request(client.as_ref(), Policy::new(), simple_request());
        assert!(
            collect(client.as_ref(), &request)
                .await
                .iter()
                .any(|result| stream_error(result).is_some())
        );
        assert_eq!(server.bodies.lock().unwrap().len(), 1);
        assert!(destination.bodies.lock().unwrap().is_empty(), "{route:?}");
    }
}

#[tokio::test]
async fn explicit_auth_retry_reprepares_and_refuses_after_revocation() {
    for route in ROUTES {
        let server = serve(StatusCode::UNAUTHORIZED, None, false, None).await;
        let policy = Policy::new();
        let authorizer = Authorizer::new(false, Some(Arc::clone(&policy.allowed)));
        let client = client(route, &server.url, Arc::clone(&authorizer));
        let request = request(client.as_ref(), Arc::clone(&policy), simple_request());
        assert!(
            refused(&collect(client.as_ref(), &request).await),
            "{route:?}"
        );
        assert_eq!(server.bodies.lock().unwrap().len(), 1);
        assert_eq!(authorizer.calls.load(Ordering::SeqCst), 1);
        let bindings = policy.bindings.lock().unwrap();
        assert_eq!(bindings.len(), 2);
        assert!(!bindings[0].same_operation(&bindings[1]));
    }
}

fn continuation_request() -> LlmRequest {
    LlmRequest::new(
        MODEL,
        vec![
            Message::User(UserMessage::text("old question")),
            Message::BlockAssistant(BlockAssistantMessage::new(
                vec![AssistantBlock::Text {
                    text: "old answer".to_owned(),
                    meta: Some(Box::new(ProviderMeta::OpenAiResponse {
                        response_id: "resp_missing".to_owned(),
                    })),
                }],
                StopReason::EndTurn,
            )),
            Message::User(UserMessage::text("new question")),
        ],
    )
    .with_openai_tag_merge(|tag| tag.store = Some(true))
}

#[tokio::test]
async fn continuation_fallback_gets_a_fresh_binding_and_cannot_bypass_revocation() {
    for revoke in [false, true] {
        let policy = Policy::new();
        let server = serve(
            StatusCode::OK,
            None,
            true,
            revoke.then(|| Arc::clone(&policy.allowed)),
        )
        .await;
        let client = client(Route::Public, &server.url, Authorizer::new(false, None));
        let request = request(client.as_ref(), Arc::clone(&policy), continuation_request());
        let events = collect(client.as_ref(), &request).await;
        assert_eq!(refused(&events), revoke);
        if !revoke {
            assert!(succeeded(&events));
        }
        let bodies = server.bodies.lock().unwrap();
        assert_eq!(bodies.len(), if revoke { 1 } else { 2 });
        assert!(bodies[0].1.get("previous_response_id").is_some());
        if !revoke {
            assert!(bodies[1].1.get("previous_response_id").is_none());
        }
        let bindings = policy.bindings.lock().unwrap();
        assert_eq!(bindings.len(), 2);
        assert!(!bindings[0].same_operation(&bindings[1]));
    }
}

#[tokio::test]
async fn prepared_request_without_selected_target_refuses_before_authentication_or_send() {
    let server = serve(StatusCode::OK, None, false, None).await;
    let authorizer = Authorizer::new(false, None);
    let client = crate::OpenAiClient::new_with_base_url("unused".to_owned(), server.url.clone())
        .with_authorizer(authorizer.clone());
    let policy = Policy::new();
    let request = request(&client, Arc::clone(&policy), simple_request());
    assert!(refused(&collect(&client, &request).await));
    assert!(server.bodies.lock().unwrap().is_empty());
    assert_eq!(authorizer.calls.load(Ordering::SeqCst), 0);
    assert!(policy.facts().is_empty());
}

#[tokio::test]
async fn changed_logical_model_cannot_reuse_the_selected_target() {
    let server = serve(StatusCode::OK, None, false, None).await;
    let policy = Policy::new();
    let authorizer = Authorizer::new(false, None);
    let client = client(Route::Public, &server.url, Arc::clone(&authorizer));
    let request = request(client.as_ref(), Arc::clone(&policy), simple_request());
    let mut changed = request.request().clone();
    changed.model = "gpt-5.3".to_owned();
    let changed = request.with_lowered_request(changed);
    assert!(refused(&collect(client.as_ref(), &changed).await));
    assert!(server.bodies.lock().unwrap().is_empty());
    assert_eq!(authorizer.calls.load(Ordering::SeqCst), 0);
    assert!(policy.facts().is_empty());
}

#[tokio::test]
async fn raw_legacy_send_does_not_acquire_a_work_context() {
    let server = serve(StatusCode::OK, None, false, None).await;
    let client = crate::OpenAiClient::new_with_base_url("unused".to_owned(), server.url.clone());
    let request = simple_request();
    let events: Vec<_> =
        tokio::time::timeout(Duration::from_secs(5), client.stream(&request).collect())
            .await
            .unwrap();
    assert!(succeeded(&events));
    assert_eq!(server.bodies.lock().unwrap().len(), 1);
}

#[test]
fn governed_wire_projection_has_no_raw_or_missing_model_fallback() {
    let client = crate::OpenAiClient::new("unused".to_owned());
    let prepared = request(&client, Policy::new(), simple_request());
    assert!(matches!(
        super::prepare_wire_authorization(
            Some(&prepared),
            "http://127.0.0.1/responses",
            &json!({"tools":[]})
        ),
        Err(LlmError::OperationRefused { .. })
    ));
    // An ungoverned compatibility request does not introduce a new validation.
    assert!(
        super::prepare_wire_authorization(None, "http://127.0.0.1/responses", &json!({}))
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn hosted_capabilities_are_projected_from_actual_lowered_body() {
    let server = serve(StatusCode::OK, None, false, None).await;
    let policy = Policy::new();
    let client = client(Route::Public, &server.url, Authorizer::new(false, None));
    let raw = simple_request()
        .with_tools(vec![Arc::new(meerkat_core::ToolDef::new(
            "local_tool",
            "not provider hosted",
            json!({"type":"object"}),
        ))])
        .with_openai_tag_merge(|tag| {
            tag.web_search = Some(
                meerkat_core::lifecycle::run_primitive::OpaqueProviderBody::from_value(
                    &json!({"type":"web_search_preview"}),
                ),
            );
        });
    let request = request(client.as_ref(), Arc::clone(&policy), raw);
    assert!(succeeded(&collect(client.as_ref(), &request).await));
    assert_eq!(
        policy.facts()[0].hosted_capabilities.as_ref(),
        &[ServerToolKind::WebSearch]
    );
    let bodies = server.bodies.lock().unwrap();
    assert_eq!(bodies[0].1["tools"][0]["type"], "function");
    assert_eq!(bodies[0].1["tools"][1]["type"], "web_search_preview");
}

#[tokio::test]
async fn fresh_current_decision_is_retained_through_actual_http_entry() {
    for route in [Route::Public, Route::CompatibleChat] {
        let policy = Arc::new(Policy {
            observations: Arc::new(Mutex::new(Vec::new())),
            fail_entry: Arc::new(AtomicBool::new(false)),
            fail_outcome: Arc::new(AtomicBool::new(false)),
            allowed: Arc::new(AtomicBool::new(true)),
            bindings: Mutex::new(Vec::new()),
            refresh_on_entry: true,
            refreshed_dropped: Arc::new(AtomicBool::new(false)),
        });
        let observed_early_drop = Arc::new(AtomicBool::new(true));
        let observed = Arc::clone(&observed_early_drop);
        let dropped = Arc::clone(&policy.refreshed_dropped);
        let app = Router::new().fallback(move |uri: Uri, Json(_body): Json<Value>| {
            let observed = Arc::clone(&observed);
            let dropped = Arc::clone(&dropped);
            async move {
                observed.store(dropped.load(Ordering::SeqCst), Ordering::SeqCst);
                let body = if uri.path().ends_with("chat/completions") {
                    CHAT_SSE
                } else {
                    RESPONSES_SSE
                };
                ([("content-type", "text/event-stream")], body)
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server = Server {
            url: format!("http://{}", listener.local_addr().unwrap()),
            bodies: Arc::new(Mutex::new(Vec::new())),
            task: tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }),
        };
        let client = client(route, &server.url, Authorizer::new(false, None));
        let request = request(client.as_ref(), Arc::clone(&policy), simple_request());
        assert!(succeeded(&collect(client.as_ref(), &request).await));
        assert!(!observed_early_drop.load(Ordering::SeqCst), "{route:?}");
        assert!(policy.refreshed_dropped.load(Ordering::SeqCst));
        assert_eq!(policy.bindings.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn configured_query_is_refused_before_policy_facts_credentials_or_network() {
    for route in ROUTES {
        let server = serve(StatusCode::OK, None, false, None).await;
        let policy = Policy::new();
        let authorizer = Authorizer::new(false, None);
        let client = client(
            route,
            &format!("{}?fixture_secret=not-for-policy", server.url),
            Arc::clone(&authorizer),
        );
        let request = request(client.as_ref(), Arc::clone(&policy), simple_request());
        assert!(
            refused(&collect(client.as_ref(), &request).await),
            "{route:?}"
        );
        assert!(policy.facts().is_empty());
        assert_eq!(authorizer.calls.load(Ordering::SeqCst), 0);
        assert!(server.bodies.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn http_audit_records_entry_and_status_and_preserves_success_if_outcome_staging_fails() {
    for route in ROUTES {
        for fail_outcome in [false, true] {
            let server = serve(StatusCode::OK, None, false, None).await;
            let policy = Policy::new();
            policy.fail_outcome.store(fail_outcome, Ordering::SeqCst);
            let client = client(route, &server.url, Authorizer::new(false, None));
            let request = request(client.as_ref(), Arc::clone(&policy), simple_request());
            let results = collect(client.as_ref(), &request).await;
            assert!(
                results.iter().all(|event| stream_error(event).is_none()),
                "{results:?}"
            );
            assert!(matches!(
                results.last(),
                Some(Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { .. }
                }))
            ));
            assert_eq!(server.bodies.lock().unwrap().len(), 1);
            let observations = policy.observations.lock().unwrap();
            assert!(matches!(
                observations.as_slice(),
                [
                    OperationObservation::Entry,
                    OperationObservation::Outcome(OperationObservedOutcome::HttpResponse {
                        status: 200
                    })
                ]
            ));
            assert_eq!(
                results
                    .iter()
                    .filter(|event| matches!(
                        event,
                        Ok(LlmEvent::OperationObservationFailed { .. })
                    ))
                    .count(),
                usize::from(fail_outcome)
            );
        }
    }
}

#[tokio::test]
async fn failed_entry_audit_prevents_physical_http_request() {
    for route in ROUTES {
        let server = serve(StatusCode::OK, None, false, None).await;
        let policy = Policy::new();
        policy.fail_entry.store(true, Ordering::SeqCst);
        let client = client(route, &server.url, Authorizer::new(false, None));
        let request = request(client.as_ref(), Arc::clone(&policy), simple_request());
        let results = collect(client.as_ref(), &request).await;
        assert!(
            results
                .iter()
                .filter_map(stream_error)
                .any(|error| matches!(error, LlmError::OperationObservationUnavailable))
        );
        assert!(!refused(&results));
        assert!(server.bodies.lock().unwrap().is_empty());
        assert!(matches!(
            policy.observations.lock().unwrap().as_slice(),
            [OperationObservation::Entry]
        ));
        assert!(
            !results
                .iter()
                .any(|event| matches!(event, Ok(LlmEvent::OperationObservationFailed { .. })))
        );
    }
}

#[tokio::test]
async fn transport_failure_retains_original_error_beside_outcome_diagnostic() {
    for route in ROUTES {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let failing_url = format!("http://{}", listener.local_addr().unwrap());
        let policy = Policy::new();
        policy.fail_outcome.store(true, Ordering::SeqCst);
        let client = client(route, &failing_url, Authorizer::new(false, None));
        let request = request(client.as_ref(), Arc::clone(&policy), simple_request());
        // Keep the port owned until collection finishes. Releasing it before
        // connecting lets another parallel test receive this request.
        let break_transport = async {
            use tokio::io::AsyncReadExt;
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 1024];
            assert!(stream.read(&mut bytes).await.unwrap() > 0);
            // Close after request entry, without sending an HTTP response.
        };
        let (results, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(collect(client.as_ref(), &request), break_transport)
        })
        .await
        .unwrap();
        assert!(
            results
                .iter()
                .filter_map(stream_error)
                .any(|error| !matches!(error, LlmError::OperationRefused { .. }))
        );
        assert_eq!(
            results
                .iter()
                .filter(|event| matches!(event, Ok(LlmEvent::OperationObservationFailed { .. })))
                .count(),
            1
        );
        let diagnostic = results
            .iter()
            .position(|event| matches!(event, Ok(LlmEvent::OperationObservationFailed { .. })))
            .unwrap();
        let failure = results
            .iter()
            .position(|event| stream_error(event).is_some())
            .unwrap();
        assert!(
            diagnostic < failure,
            "original transport error must not hide the diagnostic"
        );
        assert!(matches!(
            policy.observations.lock().unwrap().as_slice(),
            [
                OperationObservation::Entry,
                OperationObservation::Outcome(OperationObservedOutcome::TransportError)
            ]
        ));
    }
}

#[tokio::test]
async fn plain_controller_facts_match_actual_openai_and_compatible_routes_without_preflight_io() {
    for route in ROUTES {
        let server = serve(StatusCode::OK, None, false, None).await;
        let policy = Policy::new();
        let authorizer = Authorizer::new(false, None);
        let raw = client(route, &server.url, Arc::clone(&authorizer));
        let adapter = Arc::new(
            meerkat_llm_core::LlmClientAdapter::try_for_provider_identity(
                Arc::clone(&raw),
                MODEL.to_owned(),
                Provider::OpenAI,
            )
            .unwrap(),
        );
        let pinned = meerkat_core::AgentLlmClient::pin_controller(adapter).unwrap();
        let plain = pinned
            .plain_facts()
            .expect("actual immutable provider route");
        assert_eq!(plain.selection(), pinned.selection());
        assert_eq!(plain.selection().model(), MODEL);
        assert_eq!(
            plain.wire_model(),
            route.wire_model(),
            "configured remote alias must be lowered"
        );
        assert!(server.bodies.lock().unwrap().is_empty());
        assert_eq!(authorizer.calls.load(Ordering::SeqCst), 0);
        assert!(
            policy.facts().is_empty(),
            "no fabricated admission operation"
        );
        assert!(raw.plain_model_route("not-the-selected-model").is_err());

        let request = request(raw.as_ref(), Arc::clone(&policy), simple_request());
        let events = collect(raw.as_ref(), &request).await;
        assert!(succeeded(&events), "actual route {route:?}");
        let bodies = server.bodies.lock().unwrap();
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0].0, route.path());
        assert_eq!(bodies[0].1["model"], plain.wire_model());
        assert!(authorizer.calls.load(Ordering::SeqCst) > 0);
        let observed = policy.facts();
        assert_eq!(observed.len(), 1);
        assert_eq!(plain.endpoint(), observed[0].endpoint.as_ref());
        assert_eq!(plain.wire_model(), observed[0].wire_model.as_ref());
        assert!(plain.selection().matches_model_facts(&observed[0]));
    }
}

// Governed fallback search: the helper is its own model operation. The tool's
// outer admission authorizes none of the helper's model, account, endpoint or
// hosted search; the executor prepares them on the helper's selected target.

fn helper_binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse("test-realm").unwrap(),
        binding: BindingId::parse("helper-binding").unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

/// Denies only model operations attributed to one binding, so one work
/// context can allow the controller route while refusing the helper route.
struct DenyBinding {
    inner: Arc<Policy>,
    denied: AuthBindingRef,
}
impl WorkAuthorization for DenyBinding {
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, meerkat_core::OperationAuthorizationError>
    {
        if let AuthorizationOperation::Model(facts) = &binding.facts().operation
            && facts.identity.auth_binding.as_ref() == Some(&self.denied)
        {
            self.inner.bindings.lock().unwrap().push(binding.clone());
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        self.inner.prepare(binding)
    }
}

fn helper_executor(
    url: &str,
    authorizer: Arc<Authorizer>,
) -> crate::web_search::OpenAiWebSearchExecutor {
    let adapted: Arc<dyn meerkat_core::AgentLlmClient> = Arc::new(
        meerkat_llm_core::LlmClientAdapter::try_for_provider_identity(
            client_for_binding(Route::Public, url, authorizer, helper_binding()),
            MODEL.to_owned(),
            Provider::OpenAI,
        )
        .unwrap(),
    );
    crate::web_search::OpenAiWebSearchExecutor::new(MODEL.to_owned(), adapted)
}

fn search_request() -> meerkat_core::web_search::WebSearchRequest {
    meerkat_core::web_search::WebSearchRequest {
        query: "fixture query".to_owned(),
        provider: None,
        provider_params: None,
        context: None,
    }
}

fn helper_authorization(work: &WorkAuthorizationContext) -> Option<LlmRequestAuthorization> {
    Some(LlmRequestAuthorization::new(
        work.clone(),
        OperationId::new(),
        ModelAuthorizationUse::Inference,
    ))
}

fn assert_helper_attribution(facts: &ModelAuthorizationFacts) {
    assert_eq!(facts.identity.auth_binding, Some(helper_binding()));
    assert_eq!(
        facts.credential,
        Some(AuthCredentialIdentity::Binding(helper_binding()))
    );
    assert_ne!(facts.identity.auth_binding, Some(binding()));
    assert_eq!(
        facts.hosted_capabilities.as_ref(),
        &[ServerToolKind::WebSearch]
    );
    assert!(matches!(facts.usage, ModelAuthorizationUse::Inference));
}

#[tokio::test]
async fn helper_deny_sends_zero_bodies_while_the_controller_route_continues() {
    use meerkat_llm_core::WebSearchExecutor;

    let server = serve(StatusCode::OK, None, false, None).await;
    let recorded = Policy::new();
    let work = WorkAuthorizationContext::new(
        Arc::new(DenyBinding {
            inner: Arc::clone(&recorded),
            denied: helper_binding(),
        }),
        meerkat_core::exact_operation::OperationExecutionScope::Domain,
    );
    let helper_authorizer = Authorizer::new(false, None);
    let executor = helper_executor(&server.url, Arc::clone(&helper_authorizer));

    let error = executor
        .execute_web_search_authorized(search_request(), helper_authorization(&work))
        .await
        .expect_err("a denied helper route must not search");
    assert!(
        matches!(&error, LlmError::OperationRefused { refusal } if refusal.kind() == OperationRefusalKind::Denied),
        "{error:?}"
    );
    assert!(
        server.bodies.lock().unwrap().is_empty(),
        "zero helper sends"
    );
    assert_eq!(helper_authorizer.calls.load(Ordering::SeqCst), 0);
    let facts = recorded.facts();
    assert_eq!(facts.len(), 1);
    assert_helper_attribution(&facts[0]);

    // The controller route under the same work context is not poisoned.
    let controller = client(Route::Public, &server.url, Authorizer::new(false, None));
    let projection = controller
        .project_replay_request(&simple_request().messages)
        .unwrap();
    let request = PreparedLlmRequest::from_projection(simple_request(), projection)
        .with_authorization(helper_authorization(&work));
    assert!(succeeded(&collect(controller.as_ref(), &request).await));
    assert_eq!(server.bodies.lock().unwrap().len(), 1);
    let facts = recorded.facts();
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[1].identity.auth_binding, Some(binding()));
}

#[tokio::test]
async fn allowed_helper_search_is_attributed_to_its_own_route() {
    use meerkat_llm_core::WebSearchExecutor;

    let server = serve(StatusCode::OK, None, false, None).await;
    let policy = Policy::new();
    let work = WorkAuthorizationContext::new(
        Arc::clone(&policy) as Arc<dyn WorkAuthorization>,
        meerkat_core::exact_operation::OperationExecutionScope::Domain,
    );
    let executor = helper_executor(&server.url, Authorizer::new(false, None));
    executor
        .execute_web_search_authorized(search_request(), helper_authorization(&work))
        .await
        .expect("an allowed helper route searches");
    let bodies = server.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0].1["model"], MODEL);
    assert!(
        bodies[0].1["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| tool["type"] == "web_search"))
    );
    let facts = policy.facts();
    assert_eq!(facts.len(), 1);
    assert_helper_attribution(&facts[0]);
    assert_eq!(facts[0].identity.model, MODEL);
}

#[tokio::test]
async fn helper_revocation_while_credentials_are_awaited_sends_nothing() {
    use meerkat_llm_core::WebSearchExecutor;

    let server = serve(StatusCode::OK, None, false, None).await;
    let policy = Policy::new();
    let work = WorkAuthorizationContext::new(
        Arc::clone(&policy) as Arc<dyn WorkAuthorization>,
        meerkat_core::exact_operation::OperationExecutionScope::Domain,
    );
    let authorizer = Authorizer::new(true, None);
    let executor = helper_executor(&server.url, Arc::clone(&authorizer));
    let authorization = helper_authorization(&work);
    let task = tokio::spawn(async move {
        executor
            .execute_web_search_authorized(search_request(), authorization)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), authorizer.entered.notified())
        .await
        .unwrap();
    policy.allowed.store(false, Ordering::SeqCst);
    authorizer.resume.notify_one();
    let error = task
        .await
        .unwrap()
        .expect_err("currentness is rechecked after the credential wait");
    assert!(
        matches!(error, LlmError::OperationRefused { .. }),
        "{error:?}"
    );
    assert!(server.bodies.lock().unwrap().is_empty());
    assert_eq!(policy.facts().len(), 1);
}
