//! Gemini selected-target and final HTTP boundary regressions.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use futures::StreamExt;
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse, OperationObservation,
    OperationObservationError, OperationObservedOutcome, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, PreparedOperationAuthorization, WorkAuthorization,
    WorkAuthorizationContext,
};
use meerkat_core::{
    AuthBindingRef, AuthCredentialIdentity, AuthMetadata, BackendProfile, BindingId, BindingOrigin,
    Config, HttpAuthorizer, LlmRequestAuthorization, Message, ModelRegistry, OperationId, Provider,
    RealmId, SessionLlmIdentity, UserMessage,
};
use meerkat_llm_core::provider_runtime::binding::{
    DynamicLease, NormalizedBackendKind, ResolvedConnection, ResolvedTextTarget,
};
use meerkat_llm_core::provider_runtime::registry::ProviderRuntimeRegistry;
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, PreparedLlmRequest,
};
use serde_json::Value;
use tokio::sync::Notify;

const MODEL: &str = "gemini-3.5-flash";
const SSE: &str = concat!(
    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\"}],",
    "\"usageMetadata\":{\"promptTokenCount\":1,\"candidatesTokenCount\":1}}\n\n",
);
const PATH: &str = "/v1beta/models/gemini-3.5-flash:streamGenerateContent";

struct Policy {
    observations: Arc<Mutex<Vec<OperationObservation>>>,
    fail_entry: Arc<AtomicBool>,
    fail_outcome: Arc<AtomicBool>,
    allowed: Arc<AtomicBool>,
    facts: Mutex<Vec<ModelAuthorizationFacts>>,
}
impl Policy {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            observations: Arc::new(Mutex::new(Vec::new())),
            fail_entry: Arc::new(AtomicBool::new(false)),
            fail_outcome: Arc::new(AtomicBool::new(false)),
            allowed: Arc::new(AtomicBool::new(true)),
            facts: Mutex::new(Vec::new()),
        })
    }
}
struct Check {
    observations: Arc<Mutex<Vec<OperationObservation>>>,
    fail_entry: Arc<AtomicBool>,
    fail_outcome: Arc<AtomicBool>,
    allowed: Arc<AtomicBool>,
    binding: PreparedAuthorizationBinding,
}
impl PreparedOperationAuthorization for Check {
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
        Ok(())
    }
}
impl WorkAuthorization for Policy {
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, meerkat_core::OperationAuthorizationError>
    {
        let AuthorizationOperation::Model(facts) = &binding.facts().operation else {
            return Err(OperationRefused::new(OperationRefusalKind::MalformedFacts).into());
        };
        self.facts.lock().unwrap().push(facts.clone());
        if !self.allowed.load(Ordering::SeqCst) {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        Ok(Arc::new(Check {
            observations: Arc::clone(&self.observations),
            fail_entry: Arc::clone(&self.fail_entry),
            fail_outcome: Arc::clone(&self.fail_outcome),
            allowed: Arc::clone(&self.allowed),
            binding: binding.clone(),
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
    fn label(&self) -> &str {
        "authorization-boundary-test"
    }
}

#[derive(Clone)]
struct ServerState {
    bodies: Arc<Mutex<Vec<Value>>>,
    status: StatusCode,
    redirect: Option<String>,
}
struct Server {
    url: String,
    bodies: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(status: StatusCode, redirect: Option<String>) -> Server {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let state = ServerState {
        bodies: Arc::clone(&bodies),
        status,
        redirect,
    };
    let app = Router::new()
        .route(
            PATH,
            post(
                |State(state): State<ServerState>, Json(body): Json<Value>| async move {
                    state.bodies.lock().unwrap().push(body);
                    let mut response = (state.status, [("content-type", "text/event-stream")], SSE)
                        .into_response();
                    if let Some(location) = state.redirect {
                        response
                            .headers_mut()
                            .insert("location", location.parse().unwrap());
                    }
                    response
                },
            ),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server { url, bodies, task }
}

fn binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse("test-realm").unwrap(),
        binding: BindingId::parse("test-binding").unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}
fn client(url: &str, authorizer: Arc<Authorizer>) -> Arc<dyn LlmClient> {
    let binding = binding();
    let identity = SessionLlmIdentity {
        model: MODEL.to_owned(),
        provider: Provider::Gemini,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(binding.clone()),
    };
    let models =
        ModelRegistry::from_config(&Config::default(), meerkat_models::canonical()).unwrap();
    let profile = models
        .profile_witness_for_provider(Provider::Gemini, MODEL)
        .unwrap();
    let connection = ResolvedConnection {
        provider: Provider::Gemini,
        backend: NormalizedBackendKind::Google(crate::GoogleBackendKind::GoogleGenAi),
        backend_profile: Arc::new(BackendProfile {
            id: "test-backend".to_owned(),
            provider: Provider::Gemini,
            backend_kind: "google_genai".to_owned(),
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
    ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(crate::GoogleProviderRuntime))
        .build_text_client(ResolvedTextTarget::new(identity, profile, connection).unwrap())
        .unwrap()
}
fn request(client: &dyn LlmClient, policy: Arc<Policy>) -> PreparedLlmRequest {
    let messages = vec![Message::User(UserMessage::text("hello"))];
    let projection = client.project_replay_request(&messages).unwrap();
    let work = WorkAuthorizationContext::new(
        policy,
        meerkat_core::exact_operation::OperationExecutionScope::Domain,
    );
    PreparedLlmRequest::from_projection(LlmRequest::new(MODEL, messages), projection)
        .with_authorization(Some(LlmRequestAuthorization::new(
            work,
            OperationId::new(),
            ModelAuthorizationUse::Inference,
        )))
}
async fn collect(
    client: &dyn LlmClient,
    request: &PreparedLlmRequest,
) -> Vec<Result<meerkat_llm_core::LlmEvent, LlmError>> {
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
async fn selected_target_reaches_policy_and_actual_http_body() {
    let server = serve(StatusCode::OK, None).await;
    let policy = Policy::new();
    let client = client(&server.url, Authorizer::new(false, None));
    let request = request(client.as_ref(), Arc::clone(&policy));
    let results = collect(client.as_ref(), &request).await;
    assert!(succeeded(&results));
    assert_eq!(server.bodies.lock().unwrap().len(), 1);
    assert_eq!(
        server.bodies.lock().unwrap()[0]["contents"][0]["parts"][0]["text"],
        "hello"
    );
    let facts = policy.facts.lock().unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].wire_model.as_ref(), MODEL);
    assert_eq!(
        facts[0].endpoint.as_ref(),
        format!("{}{PATH}?alt=sse", server.url)
    );
    assert_eq!(
        facts[0].credential,
        Some(AuthCredentialIdentity::Binding(binding()))
    );
    assert!(facts[0].hosted_capabilities.is_empty());
}

#[tokio::test]
async fn revocation_during_authorization_await_prevents_http_send() {
    let server = serve(StatusCode::OK, None).await;
    let policy = Policy::new();
    let authorizer = Authorizer::new(true, None);
    let client = client(&server.url, Arc::clone(&authorizer));
    let request = request(client.as_ref(), Arc::clone(&policy));
    let task = tokio::spawn(async move { collect(client.as_ref(), &request).await });
    tokio::time::timeout(Duration::from_secs(5), authorizer.entered.notified())
        .await
        .unwrap();
    policy.allowed.store(false, Ordering::SeqCst);
    authorizer.resume.notify_one();
    assert!(refused(&task.await.unwrap()));
    assert!(server.bodies.lock().unwrap().is_empty());
    assert_eq!(policy.facts.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn checked_target_never_follows_a_redirect() {
    let destination = serve(StatusCode::OK, None).await;
    let server = serve(
        StatusCode::TEMPORARY_REDIRECT,
        Some(format!("{}{PATH}?alt=sse", destination.url)),
    )
    .await;
    let client = client(&server.url, Authorizer::new(false, None));
    let request = request(client.as_ref(), Policy::new());
    let results = collect(client.as_ref(), &request).await;
    assert!(results.iter().any(|result| stream_error(result).is_some()));
    assert_eq!(server.bodies.lock().unwrap().len(), 1);
    assert!(destination.bodies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn http_audit_records_entry_and_status_and_preserves_success_if_outcome_staging_fails() {
    {
        for fail_outcome in [false, true] {
            let server = serve(StatusCode::OK, None).await;
            let policy = Policy::new();
            policy.fail_outcome.store(fail_outcome, Ordering::SeqCst);
            let client = client(&server.url, Authorizer::new(false, None));
            let request = request(client.as_ref(), Arc::clone(&policy));
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
    {
        let server = serve(StatusCode::OK, None).await;
        let policy = Policy::new();
        policy.fail_entry.store(true, Ordering::SeqCst);
        let client = client(&server.url, Authorizer::new(false, None));
        let request = request(client.as_ref(), Arc::clone(&policy));
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
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let unused_url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let policy = Policy::new();
        policy.fail_outcome.store(true, Ordering::SeqCst);
        let client = client(&unused_url, Authorizer::new(false, None));
        let request = request(client.as_ref(), Arc::clone(&policy));
        let results = collect(client.as_ref(), &request).await;
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

// API-dependent controller-admission facts test. This performs no native
// admission; the existing real provider HTTP owner proves route equivalence.
#[tokio::test]
async fn plain_controller_facts_match_actual_gemini_request_without_preflight_io() {
    let server = serve(StatusCode::OK, None).await;
    let policy = Policy::new();
    let authorizer = Authorizer::new(false, None);
    let raw = client(&server.url, Arc::clone(&authorizer));
    let adapter = Arc::new(
        meerkat_llm_core::LlmClientAdapter::try_for_provider_identity(
            Arc::clone(&raw),
            MODEL.to_owned(),
            Provider::Gemini,
        )
        .unwrap(),
    );
    let pinned = meerkat_core::AgentLlmClient::pin_controller(adapter).unwrap();
    let plain = pinned
        .plain_facts()
        .expect("actual immutable provider route");
    assert_eq!(plain.selection(), pinned.selection());
    assert!(
        server.bodies.lock().unwrap().is_empty(),
        "facts must not send a request"
    );
    assert_eq!(
        authorizer.calls.load(Ordering::SeqCst),
        0,
        "facts must not acquire credentials"
    );
    assert!(
        policy.facts.lock().unwrap().is_empty(),
        "facts must not fabricate an operation"
    );
    assert!(raw.plain_model_route("not-the-selected-model").is_err());
    assert_eq!(authorizer.calls.load(Ordering::SeqCst), 0);

    let request = request(raw.as_ref(), Arc::clone(&policy));
    let events = collect(raw.as_ref(), &request).await;
    assert!(events.iter().all(|event| stream_error(event).is_none()));
    assert!(matches!(
        events.last(),
        Some(Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Success { .. }
        }))
    ));
    assert_eq!(
        server.bodies.lock().unwrap().len(),
        1,
        "positive actual HTTP control"
    );
    assert!(authorizer.calls.load(Ordering::SeqCst) > 0);
    let observed = policy.facts.lock().unwrap();
    assert_eq!(observed.len(), 1);
    assert_eq!(plain.endpoint(), observed[0].endpoint.as_ref());
    assert_eq!(plain.wire_model(), observed[0].wire_model.as_ref());
    assert!(plain.selection().matches_model_facts(&observed[0]));
    assert!(observed[0].hosted_capabilities.is_empty());
}
