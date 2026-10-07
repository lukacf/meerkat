//! Actual selected-target and final HTTP boundary regressions.
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
    AgentLlmClient, AuthBindingRef, AuthCredentialIdentity, AuthMetadata, BackendProfile,
    BindingId, BindingOrigin, Config, HttpAuthorizer, LlmRequestAuthorization, Message,
    ModelRegistry, OperationId, Provider, RealmId, SessionLlmIdentity, ToolChoice, ToolDef,
    UserMessage,
};
use meerkat_llm_core::provider_runtime::binding::{
    DynamicLease, NormalizedBackendKind, ResolvedConnection, ResolvedTextTarget,
};
use meerkat_llm_core::provider_runtime::registry::ProviderRuntimeRegistry;
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, PreparedLlmRequest,
    ToolChoiceRefusal,
};
use serde_json::Value;
use tokio::sync::Notify;

const MODEL: &str = "claude-sonnet-4-6";
const SSE: &str = concat!(
    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n",
    "data: {\"type\":\"content_block_start\",\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n",
    "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n",
    "data: {\"type\":\"content_block_stop\"}\n",
    "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":1},\"delta\":{\"stop_reason\":\"end_turn\"}}\n",
    "data: {\"type\":\"message_stop\"}\n\n",
);

struct Policy {
    observations: Arc<Mutex<Vec<OperationObservation>>>,
    fail_entry: Arc<AtomicBool>,
    fail_outcome: Arc<AtomicBool>,
    fail_refused: Arc<AtomicBool>,
    revoke_on_entry: Arc<AtomicBool>,
    allowed: Arc<AtomicBool>,
    unavailable: Arc<AtomicBool>,
    facts: Mutex<Vec<ModelAuthorizationFacts>>,
}
impl Policy {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            observations: Arc::new(Mutex::new(Vec::new())),
            fail_entry: Arc::new(AtomicBool::new(false)),
            fail_outcome: Arc::new(AtomicBool::new(false)),
            fail_refused: Arc::new(AtomicBool::new(false)),
            revoke_on_entry: Arc::new(AtomicBool::new(false)),
            allowed: Arc::new(AtomicBool::new(true)),
            unavailable: Arc::new(AtomicBool::new(false)),
            facts: Mutex::new(Vec::new()),
        })
    }
}
struct Check {
    observations: Arc<Mutex<Vec<OperationObservation>>>,
    fail_entry: Arc<AtomicBool>,
    fail_outcome: Arc<AtomicBool>,
    fail_refused: Arc<AtomicBool>,
    revoke_on_entry: Arc<AtomicBool>,
    allowed: Arc<AtomicBool>,
    unavailable: Arc<AtomicBool>,
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
        let entry = matches!(&observation, OperationObservation::Entry);
        let fail = match &observation {
            OperationObservation::Entry => self.fail_entry.load(Ordering::SeqCst),
            OperationObservation::Outcome(_) => self.fail_outcome.load(Ordering::SeqCst),
            OperationObservation::AuthorizationUnavailable => false,
            OperationObservation::Refused(_) => self.fail_refused.load(Ordering::SeqCst),
        };
        self.observations.lock().unwrap().push(observation);
        if fail {
            return Err(OperationObservationError);
        }
        if entry && self.revoke_on_entry.load(Ordering::SeqCst) {
            self.allowed.store(false, Ordering::SeqCst);
        }
        Ok(())
    }

    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(meerkat_core::OperationAuthorizationError::Unavailable);
        }
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
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(meerkat_core::OperationAuthorizationError::Unavailable);
        }
        if !self.allowed.load(Ordering::SeqCst) {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        Ok(Arc::new(Check {
            observations: Arc::clone(&self.observations),
            fail_entry: Arc::clone(&self.fail_entry),
            fail_outcome: Arc::clone(&self.fail_outcome),
            fail_refused: Arc::clone(&self.fail_refused),
            revoke_on_entry: Arc::clone(&self.revoke_on_entry),
            allowed: Arc::clone(&self.allowed),
            unavailable: Arc::clone(&self.unavailable),
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
    fn label(&self) -> &'static str {
        "authorization-boundary-test"
    }
}

#[derive(Clone)]
struct ServerState {
    bodies: Arc<Mutex<Vec<Value>>>,
    status: StatusCode,
    redirect: Option<String>,
    content_type: &'static str,
    response_body: &'static str,
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
    serve_response(status, redirect, "text/event-stream", SSE).await
}
async fn serve_response(
    status: StatusCode,
    redirect: Option<String>,
    content_type: &'static str,
    response_body: &'static str,
) -> Server {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let state = ServerState {
        bodies: Arc::clone(&bodies),
        status,
        redirect,
        content_type,
        response_body,
    };
    let app = Router::new()
        .route(
            "/v1/messages",
            post(
                |State(state): State<ServerState>, Json(body): Json<Value>| async move {
                    state.bodies.lock().unwrap().push(body);
                    let mut response = (
                        state.status,
                        [("content-type", state.content_type)],
                        state.response_body,
                    )
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
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(binding.clone()),
    };
    let models =
        ModelRegistry::from_config(&Config::default(), meerkat_models::canonical()).unwrap();
    let profile = models
        .profile_witness_for_provider(Provider::Anthropic, MODEL)
        .unwrap();
    let connection = ResolvedConnection {
        provider: Provider::Anthropic,
        backend: NormalizedBackendKind::Anthropic(crate::AnthropicBackendKind::AnthropicApi),
        backend_profile: Arc::new(BackendProfile {
            id: "test-backend".to_owned(),
            provider: Provider::Anthropic,
            backend_kind: "anthropic_api".to_owned(),
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
        .with_runtime(Arc::new(crate::AnthropicProviderRuntime))
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
    assert!(results.iter().all(|result| stream_error(result).is_none()));
    assert!(matches!(
        results.last(),
        Some(Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::Success { .. },
        }))
    ));
    assert_eq!(server.bodies.lock().unwrap().len(), 1);
    assert_eq!(server.bodies.lock().unwrap()[0]["model"], MODEL);
    let facts = policy.facts.lock().unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].wire_model.as_ref(), MODEL);
    assert_eq!(
        facts[0].endpoint.as_ref(),
        format!("{}/v1/messages", server.url)
    );
    assert_eq!(
        facts[0].credential,
        Some(AuthCredentialIdentity::Binding(binding()))
    );
    assert!(facts[0].hosted_capabilities.is_empty());
}

#[tokio::test]
async fn actual_registry_adapter_pin_reaches_the_same_provider_target() {
    let server = serve(StatusCode::OK, None).await;
    let policy = Policy::new();
    let raw = client(&server.url, Authorizer::new(false, None));
    let selected = raw.controller_model_selection().unwrap();
    let adapter = Arc::new(
        meerkat_llm_core::LlmClientAdapter::try_for_provider_identity(
            raw,
            MODEL.to_owned(),
            Provider::Anthropic,
        )
        .unwrap(),
    );
    let pinned = adapter.pin_controller().unwrap();
    assert_eq!(pinned.selection(), &selected);
    let work = WorkAuthorizationContext::new(
        policy.clone(),
        meerkat_core::exact_operation::OperationExecutionScope::Domain,
    );
    // This fixture policy tests forwarding, not native controller admission.
    // The actual admitted controller grant remains the feature owner's job.
    let authorization = LlmRequestAuthorization::new(
        work,
        OperationId::new(),
        ModelAuthorizationUse::ControllerInference,
    );
    tokio::time::timeout(
        Duration::from_secs(5),
        pinned.client().stream_response_authorized(
            &[Message::User(UserMessage::text("hello"))],
            &[],
            64,
            None,
            None,
            Some(authorization),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(server.bodies.lock().unwrap().len(), 1);
    let facts = policy.facts.lock().unwrap();
    assert_eq!(facts.len(), 1);
    assert!(selected.matches_model_facts(&facts[0]));
    assert!(facts[0].usage == ModelAuthorizationUse::ControllerInference);
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
        Some(format!("{}/v1/messages", destination.url)),
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
async fn explicit_auth_retry_rechecks_before_reusing_credential() {
    let server = serve(StatusCode::UNAUTHORIZED, None).await;
    let policy = Policy::new();
    let authorizer = Authorizer::new(false, Some(Arc::clone(&policy.allowed)));
    let client = client(&server.url, Arc::clone(&authorizer));
    let request = request(client.as_ref(), Arc::clone(&policy));
    assert!(refused(&collect(client.as_ref(), &request).await));
    assert_eq!(server.bodies.lock().unwrap().len(), 1);
    assert_eq!(authorizer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(policy.facts.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn configured_query_refuses_before_credential_use_or_target_projection() {
    let server = serve(StatusCode::OK, None).await;
    let policy = Policy::new();
    let authorizer = Authorizer::new(false, None);
    let client = client(
        &format!("{}?token=synthetic", server.url),
        Arc::clone(&authorizer),
    );
    let request = request(client.as_ref(), Arc::clone(&policy));
    assert!(refused(&collect(client.as_ref(), &request).await));
    assert_eq!(authorizer.calls.load(Ordering::SeqCst), 0);
    assert!(policy.facts.lock().unwrap().is_empty());
    assert!(server.bodies.lock().unwrap().is_empty());
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
        let failing_url = format!("http://{}", listener.local_addr().unwrap());
        let policy = Policy::new();
        policy.fail_outcome.store(true, Ordering::SeqCst);
        let client = client(&failing_url, Authorizer::new(false, None));
        let request = request(client.as_ref(), Arc::clone(&policy));
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
async fn observation_infrastructure_entry_failure_never_sends_or_enters_denial_recovery() {
    let server = serve(StatusCode::OK, None).await;
    let failed_policy = Policy::new();
    failed_policy.fail_entry.store(true, Ordering::SeqCst);
    let authorizer = Authorizer::new(false, None);
    let failed_client = client(&server.url, Arc::clone(&authorizer));
    let failed_request = request(failed_client.as_ref(), Arc::clone(&failed_policy));
    let results = collect(failed_client.as_ref(), &failed_request).await;
    assert!(
        server.bodies.lock().unwrap().is_empty(),
        "failed entry must not send"
    );
    assert_eq!(authorizer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        failed_policy.facts.lock().unwrap().len(),
        1,
        "no retry preparation"
    );
    assert!(
        matches!(
            failed_policy.observations.lock().unwrap().as_slice(),
            [OperationObservation::Entry]
        ),
        "no recursive Refused or fabricated physical Outcome observation"
    );
    let sibling_policy = Policy::new();
    let sibling_client = client(&server.url, Authorizer::new(false, None));
    let sibling_request = request(sibling_client.as_ref(), sibling_policy);
    let sibling = collect(sibling_client.as_ref(), &sibling_request).await;
    assert!(sibling.iter().all(|event| stream_error(event).is_none()));
    assert!(
        matches!(
            sibling.last(),
            Some(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
                ..
            }))
        ),
        "independent healthy request must reach actual successful completion"
    );
    assert_eq!(
        server.bodies.lock().unwrap().len(),
        1,
        "only independent sibling entered"
    );
    let error = results
        .iter()
        .find_map(stream_error)
        .expect("typed infrastructure result");
    assert!(
        !refused(&results),
        "infrastructure loss must not trigger policy feedback"
    );
    assert!(!error.is_retryable());
    let agent_error = error.clone().into_agent_error("anthropic");
    assert!(agent_error.operation_refusal().is_none());
    assert!(meerkat_core::retry::LlmRetryFailure::from_agent_error(&agent_error).is_none());
    assert!(meerkat_core::model_fallback::model_fallback_trigger(&agent_error).is_none());
}

// API-dependent regression: the common infrastructure variant is introduced
// by Slice 2. Auth/HTTP preparation uses the existing real provider fixture.
#[tokio::test]
async fn observation_infrastructure_post_entry_refusal_append_failure_never_sends() {
    let server = serve(StatusCode::OK, None).await;
    let policy = Policy::new();
    policy.revoke_on_entry.store(true, Ordering::SeqCst);
    policy.fail_refused.store(true, Ordering::SeqCst);
    let authorizer = Authorizer::new(false, None);
    let selected_client = client(&server.url, Arc::clone(&authorizer));
    let prepared_request = request(selected_client.as_ref(), Arc::clone(&policy));
    let results = collect(selected_client.as_ref(), &prepared_request).await;

    assert_eq!(
        authorizer.calls.load(Ordering::SeqCst),
        1,
        "reach the normal credential preparation exactly once"
    );
    assert_eq!(
        policy.facts.lock().unwrap().len(),
        1,
        "no retry or replacement operation preparation"
    );
    assert!(
        server.bodies.lock().unwrap().is_empty(),
        "successful Entry staging cannot bypass the following current check"
    );
    assert!(
        matches!(
            policy.observations.lock().unwrap().as_slice(),
            [
                OperationObservation::Entry,
                OperationObservation::Refused(OperationRefusalKind::Denied),
            ]
        ),
        "one staged Entry, one failed refusal attempt, no fabricated Outcome or recursion"
    );
    let error = results
        .iter()
        .find_map(stream_error)
        .expect("infrastructure result");
    assert!(matches!(error, LlmError::OperationObservationUnavailable));
    assert!(!refused(&results), "no policy-feedback classification");
    assert!(!error.is_retryable());
    let agent_error = error.clone().into_agent_error("anthropic");
    assert!(agent_error.operation_refusal().is_none());
    assert!(meerkat_core::retry::LlmRetryFailure::from_agent_error(&agent_error).is_none());
    assert!(meerkat_core::model_fallback::model_fallback_trigger(&agent_error).is_none());
}

// API-dependent controller-admission facts test. This performs no native
// admission; the existing real provider HTTP owner proves route equivalence.
#[tokio::test]
async fn plain_controller_facts_match_actual_anthropic_request_without_preflight_io() {
    let server = serve(StatusCode::OK, None).await;
    let policy = Policy::new();
    let authorizer = Authorizer::new(false, None);
    let raw = client(&server.url, Arc::clone(&authorizer));
    let adapter = Arc::new(
        meerkat_llm_core::LlmClientAdapter::try_for_provider_identity(
            Arc::clone(&raw),
            MODEL.to_owned(),
            Provider::Anthropic,
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

#[tokio::test]
async fn authority_unavailable_before_prepare_or_after_auth_never_sends() {
    for after_auth in [false, true] {
        let server = serve(StatusCode::OK, None).await;
        let policy = Policy::new();
        let authorizer = Authorizer::new(after_auth, None);
        let selected = client(&server.url, authorizer.clone());
        let prepared = request(selected.as_ref(), policy.clone());
        if !after_auth {
            policy.unavailable.store(true, Ordering::SeqCst);
        }
        let task = tokio::spawn(async move { collect(selected.as_ref(), &prepared).await });
        if after_auth {
            tokio::time::timeout(Duration::from_secs(5), authorizer.entered.notified())
                .await
                .unwrap();
            policy.unavailable.store(true, Ordering::SeqCst);
            authorizer.resume.notify_one();
        }
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert!(server.bodies.lock().unwrap().is_empty());
        assert_eq!(
            policy.facts.lock().unwrap().len(),
            1,
            "no retry preparation"
        );
        assert_eq!(
            authorizer.calls.load(Ordering::SeqCst),
            usize::from(after_auth)
        );
        assert!(matches!(
            result.iter().find_map(stream_error),
            Some(LlmError::OperationAuthorizationUnavailable)
        ));
        assert!(!refused(&result));
        assert!(
            policy
                .observations
                .lock()
                .unwrap()
                .iter()
                .all(|event| matches!(event, OperationObservation::AuthorizationUnavailable))
        );
        let next = client(&server.url, Authorizer::new(false, None));
        policy.unavailable.store(false, Ordering::SeqCst);
        let next_request = request(next.as_ref(), policy.clone());
        let healthy = collect(next.as_ref(), &next_request).await;
        assert!(matches!(
            healthy.last(),
            Some(Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::Success { .. },
                ..
            }))
        ));
        assert_eq!(
            server.bodies.lock().unwrap().len(),
            1,
            "only new explicit healthy action sends"
        );
    }
}

fn forced_request(client: &dyn LlmClient, policy: Arc<Policy>) -> PreparedLlmRequest {
    let prepared = request(client, policy);
    prepared.with_lowered_request(
        prepared
            .request()
            .clone()
            .with_tools(vec![Arc::new(ToolDef {
                name: "lookup".into(),
                description: "Synthetic lookup tool".to_owned(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
                provenance: None,
            })])
            .with_tool_choice(ToolChoice::Tool {
                name: "lookup".to_owned(),
            }),
    )
}

#[tokio::test]
async fn prepared_forced_choice_rechecks_final_authority_before_http() {
    for revoke_on_entry in [false, true] {
        let server = serve(StatusCode::OK, None).await;
        let policy = Policy::new();
        policy
            .revoke_on_entry
            .store(revoke_on_entry, Ordering::SeqCst);
        let authorizer = Authorizer::new(false, None);
        let selected = client(&server.url, Arc::clone(&authorizer));
        let prepared = forced_request(selected.as_ref(), Arc::clone(&policy));
        let events = collect(selected.as_ref(), &prepared).await;

        assert_eq!(authorizer.calls.load(Ordering::SeqCst), 1);
        let facts = policy.facts.lock().unwrap();
        assert_eq!(facts.len(), 1, "one actual selected-target preparation");
        assert_eq!(facts[0].wire_model.as_ref(), MODEL);
        assert_eq!(
            facts[0].endpoint.as_ref(),
            format!("{}/v1/messages", server.url)
        );
        assert_eq!(
            facts[0].credential,
            Some(AuthCredentialIdentity::Binding(binding()))
        );
        assert!(facts[0].hosted_capabilities.is_empty());
        let bodies = server.bodies.lock().unwrap();
        let observations = policy.observations.lock().unwrap();
        if revoke_on_entry {
            assert!(
                bodies.is_empty(),
                "no send after final authority revocation"
            );
            assert!(
                matches!(
                    events.iter().find_map(stream_error),
                    Some(LlmError::OperationRefused { refusal })
                        if refusal.kind() == OperationRefusalKind::Denied
                ),
                "{events:?}"
            );
            assert!(matches!(
                observations.as_slice(),
                [
                    OperationObservation::Entry,
                    OperationObservation::Refused(OperationRefusalKind::Denied),
                ]
            ));
        } else {
            assert!(
                events.iter().all(|event| stream_error(event).is_none()),
                "{events:?}"
            );
            assert!(matches!(
                events.last(),
                Some(Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::Success { .. }
                }))
            ));
            assert_eq!(
                bodies.len(),
                1,
                "healthy forced request reaches the same receiver"
            );
            assert_eq!(bodies[0]["model"], MODEL);
            assert_eq!(
                bodies[0]["tool_choice"],
                serde_json::json!({"type": "tool", "name": "lookup"})
            );
            assert_eq!(bodies[0]["tools"].as_array().unwrap().len(), 1);
            assert_eq!(bodies[0]["tools"][0]["name"], "lookup");
            assert!(matches!(
                observations.as_slice(),
                [
                    OperationObservation::Entry,
                    OperationObservation::Outcome(OperationObservedOutcome::HttpResponse {
                        status: 200
                    })
                ]
            ));
        }
    }
}

#[tokio::test]
async fn prepared_forced_choice_with_thinking_refuses_before_auth_or_http() {
    let server = serve(StatusCode::OK, None).await;
    let policy = Policy::new();
    let authorizer = Authorizer::new(false, None);
    let selected = client(&server.url, Arc::clone(&authorizer));
    let prepared = forced_request(selected.as_ref(), Arc::clone(&policy));
    let prepared =
        prepared.with_lowered_request(prepared.request().clone().with_anthropic_tag_merge(|tag| {
            tag.thinking =
                Some(meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig::Adaptive);
        }));
    let events = collect(selected.as_ref(), &prepared).await;
    let errors: Vec<_> = events.iter().filter_map(stream_error).collect();
    assert_eq!(errors.len(), 1, "{events:?}");
    assert!(
        matches!(
            errors[0],
            LlmError::ToolChoiceUnsupported {
                provider,
                choice: ToolChoice::Tool { name },
                reason: ToolChoiceRefusal::ForcedToolWithThinking,
            } if provider == "anthropic" && name == "lookup"
        ),
        "{events:?}"
    );
    assert!(!errors[0].is_retryable());
    assert!(
        !refused(&events),
        "a request-shape refusal is not policy denial"
    );
    assert_eq!(authorizer.calls.load(Ordering::SeqCst), 0);
    assert!(policy.facts.lock().unwrap().is_empty());
    assert!(policy.observations.lock().unwrap().is_empty());
    assert!(server.bodies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn prepared_forced_choice_provider_rejection_keeps_http_outcome_and_typed_error() {
    let server = serve_response(
        StatusCode::BAD_REQUEST,
        None,
        "application/json",
        r#"{"type":"error","error":{"type":"invalid_request_error","message":"tool_choice: type \"tool\" and \"any\" are not supported for this model."}}"#,
    )
    .await;
    let policy = Policy::new();
    let authorizer = Authorizer::new(false, None);
    let selected = client(&server.url, Arc::clone(&authorizer));
    let prepared = forced_request(selected.as_ref(), Arc::clone(&policy));
    let events = collect(selected.as_ref(), &prepared).await;
    let errors: Vec<_> = events.iter().filter_map(stream_error).collect();
    assert_eq!(errors.len(), 1, "{events:?}");
    assert!(
        matches!(
            errors[0],
            LlmError::ToolChoiceUnsupported {
                provider,
                choice: ToolChoice::Tool { name },
                reason: ToolChoiceRefusal::ModelDoesNotSupportForcedToolChoice,
            } if provider == "anthropic" && name == "lookup"
        ),
        "{events:?}"
    );
    assert!(!errors[0].is_retryable());
    assert!(
        !refused(&events),
        "provider rejection must not become policy denial"
    );
    assert!(
        events.iter().all(|event| !matches!(
            event,
            Ok(LlmEvent::ToolCallDelta { .. } | LlmEvent::ToolCallComplete { .. })
        )),
        "a rejected request must not synthesize a tool dispatch: {events:?}"
    );
    assert_eq!(authorizer.calls.load(Ordering::SeqCst), 1, "no auth retry");
    let bodies = server.bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1, "one physical rejected request, no retry");
    assert_eq!(bodies[0]["model"], MODEL);
    assert_eq!(
        bodies[0]["tool_choice"],
        serde_json::json!({"type": "tool", "name": "lookup"})
    );
    let facts = policy.facts.lock().unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].wire_model.as_ref(), MODEL);
    assert_eq!(
        facts[0].endpoint.as_ref(),
        format!("{}/v1/messages", server.url)
    );
    assert_eq!(
        facts[0].credential,
        Some(AuthCredentialIdentity::Binding(binding()))
    );
    assert!(facts[0].hosted_capabilities.is_empty());
    assert!(
        matches!(
            policy.observations.lock().unwrap().as_slice(),
            [
                OperationObservation::Entry,
                OperationObservation::Outcome(OperationObservedOutcome::HttpResponse {
                    status: 400
                })
            ]
        ),
        "the real HTTP outcome must not be rewritten as a Refused observation"
    );
}
