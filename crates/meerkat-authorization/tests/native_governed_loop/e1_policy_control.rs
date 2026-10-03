//! E1 only: real HTTP provider, one mixed tool batch, actual native owners.
//! The private receiver is deterministic; it does not stand in for a live model.
use super::*;
use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::post};
use futures::FutureExt;
use meerkat_authorization::grant_policy::ControllerAdmissionAllowance;
use meerkat_core::{
    AuthBindingRef, AuthMetadata, BackendProfile, BindingId, BindingOrigin, HttpAuthorizer,
    ModelRegistry,
};
use meerkat_llm_core::provider_runtime::binding::{
    DynamicLease, NormalizedBackendKind, ResolvedConnection, ResolvedTextTarget,
};
use meerkat_llm_core::provider_runtime::registry::ProviderRuntimeRegistry;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const E1_MODEL: &str = "claude-sonnet-4-6";
const FINISHED: &str = "read completed in the same native run";
const MAX_RECEIPT_BYTES: usize = 128 * 1024;

#[derive(Default)]
struct Receiver {
    bodies: Mutex<Vec<Value>>,
    first_responses: Vec<String>,
    second_request: Notify,
    finish: Notify,
    authorized_requests: AtomicUsize,
}

struct Server {
    base_url: String,
    receiver: Arc<Receiver>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
impl Server {
    async fn start() -> Self {
        Self::start_with_tool_response(sibling_response()).await
    }
    async fn start_with_tool_response(first_response: String) -> Self {
        Self::start_with_tool_responses(vec![first_response]).await
    }
    async fn start_with_tool_responses(first_responses: Vec<String>) -> Self {
        assert!(!first_responses.is_empty());
        let receiver = Arc::new(Receiver {
            first_responses,
            ..Receiver::default()
        });
        let app = Router::new()
            .route("/v1/messages", post(receive))
            .with_state(receiver.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base_url,
            receiver,
            task: Some(task),
        }
    }
    async fn reap(&mut self) {
        let task = self.task.take().unwrap();
        task.abort();
        match task.await {
            Err(error) if error.is_cancelled() => {}
            Ok(()) => {}
            Err(error) => panic!("recording server failed: {error}"),
        }
    }
}

fn sse(events: Vec<Value>) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::new();
    for event in events {
        write!(encoded, "data: {event}\n\n").expect("write SSE event to String");
    }
    encoded
}
fn start_message() -> Value {
    json!({"type":"message_start","message":{"id":"e1-response","type":"message","role":"assistant","model":E1_MODEL,"content":[],"stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}}})
}
fn sibling_response() -> String {
    sibling_response_with_ids(DENIED_CALL, PERMITTED_CALL)
}
fn sibling_response_with_ids(denied_call: &str, permitted_call: &str) -> String {
    let mut events = vec![start_message()];
    for (index, id, name) in [
        (0, denied_call, "delete_record"),
        (1, permitted_call, "read_record"),
    ] {
        events.extend([
            json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
            json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":"{\"record\":\"record-7\"}"}}),
            json!({"type":"content_block_stop","index":index}),
        ]);
    }
    events.extend([
        json!({"type":"message_delta","usage":{"output_tokens":2},"delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ]);
    sse(events)
}
fn final_response() -> String {
    sse(vec![
        start_message(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":FINISHED}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","usage":{"output_tokens":3},"delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_stop"}),
    ])
}
async fn receive(
    State(receiver): State<Arc<Receiver>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let count = {
        let mut bodies = receiver.bodies.lock().unwrap();
        bodies.push(body);
        bodies.len()
    };
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer synthetic-e1-loopback-only")
    {
        return (
            StatusCode::UNAUTHORIZED,
            [("content-type", "application/json")],
            "missing fixture authorization".into(),
        );
    }
    let Some(first_response) = receiver.first_responses.get((count - 1) / 2) else {
        return (
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json")],
            "unexpected extra request".into(),
        );
    };
    if count % 2 == 1 {
        return (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            first_response.clone(),
        );
    }
    receiver.second_request.notify_one();
    receiver.finish.notified().await;
    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        final_response(),
    )
}

struct FixtureAuthorizer(Arc<Receiver>);
#[async_trait]
impl HttpAuthorizer for FixtureAuthorizer {
    async fn authorize(
        &self,
        request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
    ) -> Result<(), meerkat_core::AuthError> {
        self.0.authorized_requests.fetch_add(1, Ordering::SeqCst);
        request.headers.push((
            "Authorization".into(),
            "Bearer synthetic-e1-loopback-only".into(),
        ));
        Ok(())
    }
    fn label(&self) -> &'static str {
        "e1-private-loopback"
    }
}
fn http_client(server: &Server) -> Arc<dyn LlmClient> {
    let binding = AuthBindingRef {
        realm: RealmId::parse("native-loop").unwrap(),
        binding: BindingId::parse(format!("e1-{}", SessionId::new())).unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    };
    let identity = SessionLlmIdentity {
        model: E1_MODEL.into(),
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(binding.clone()),
    };
    let models =
        ModelRegistry::from_config(&Config::default(), meerkat_models::canonical()).unwrap();
    let profile = models
        .profile_witness_for_provider(Provider::Anthropic, E1_MODEL)
        .unwrap();
    let connection = ResolvedConnection {
        provider: Provider::Anthropic,
        backend: NormalizedBackendKind::Anthropic(
            meerkat_anthropic::AnthropicBackendKind::AnthropicApi,
        ),
        backend_profile: Arc::new(BackendProfile {
            id: "e1-private-http".into(),
            provider: Provider::Anthropic,
            backend_kind: "anthropic_api".into(),
            base_url: Some(server.base_url.clone()),
            options: Value::Null,
            server: None,
        }),
        credential_identity: AuthCredentialIdentity::Binding(binding),
        auth_lease: Arc::new(DynamicLease::from_authorizer(
            Arc::new(FixtureAuthorizer(server.receiver.clone())),
            AuthMetadata::default(),
            "synthetic-e1",
        )),
    };
    ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(meerkat_anthropic::AnthropicProviderRuntime))
        .build_text_client(ResolvedTextTarget::new(identity, profile, connection).unwrap())
        .unwrap()
}

/// Real application policy seam. Both tool actions are mapped; the generated
/// operation grant alone denies delete. Controller route/account must be exact.
struct HttpRecordOwner {
    selection: ControllerModelSelection,
    endpoint: String,
}
impl HttpRecordOwner {
    fn values(&self) -> Vec<LocalOperationValues> {
        vec![LocalOperationValues {
            action: action("infer"),
            resource_domain: domain(),
            processor: ProcessorRef::Principal {
                principal: principal("executor"),
            },
            audience: AudienceRef::Principal {
                principal: principal("requester"),
            },
        }]
    }
}
impl OperationPolicyOwner for HttpRecordOwner {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        _: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        if association.candidate().controller_model.as_ref() != Some(&self.selection)
            || facts.selection() != &self.selection
            || facts.endpoint() != self.endpoint
            || facts.wire_model() != E1_MODEL
        {
            return Err(denied().into());
        }
        Ok(ControllerAdmissionAllowance {
            operation_values: self.values(),
            restrictions: ExecutionRestrictions::unrestricted(),
        })
    }
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        if matches!(&binding.facts().operation, AuthorizationOperation::Tool(_)) {
            return RecordOwner.authorize_operation(association, binding, purpose, now_ms);
        }
        let AuthorizationOperation::Model(facts) = &binding.facts().operation else {
            return Err(denied().into());
        };
        if purpose != LocalPolicyPurpose::Controller
            || !self.selection.matches_model_facts(facts)
            || facts.endpoint.as_ref() != self.endpoint
            || facts.wire_model.as_ref() != E1_MODEL
            || !facts.hosted_capabilities.is_empty()
            || facts.live_channel.is_some()
        {
            return Err(denied().into());
        }
        Ok(LocalPolicyAllowance {
            operation_values: self.values(),
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }
}

struct Evidence {
    directory: PathBuf,
}
impl Evidence {
    fn start(session_id: &SessionId, input_id: &meerkat_core::InputId) -> Self {
        let root = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
            .or_else(|| std::env::var_os("MEERKAT_E1_EVIDENCE_DIR"))
            .filter(|path| !path.is_empty())
            .expect("E1 requires an explicit test evidence directory");
        let directory = PathBuf::from(root).join(format!("adr-e1-{session_id}"));
        std::fs::create_dir_all(directory.parent().unwrap()).unwrap();
        std::fs::create_dir(&directory).expect("fresh invocation evidence directory");
        let evidence = Self { directory };
        evidence.write("started.json", &json!({"checkpoint":"E1","status":"started","session_id":session_id,"input_id":input_id}));
        evidence
    }
    fn write(&self, name: &str, value: &Value) {
        use std::io::Write;
        let bytes = serde_json::to_vec_pretty(value).unwrap();
        assert!(bytes.len() <= MAX_RECEIPT_BYTES, "bounded evidence only");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.directory.join(name))
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.write_all(b"\n").unwrap();
    }
}

fn assert_wire_sibling_feedback(body: &Value) {
    assert_wire_sibling_feedback_with_ids(body, DENIED_CALL, PERMITTED_CALL, 2);
}
fn assert_wire_sibling_feedback_with_ids(
    body: &Value,
    denied_call: &str,
    permitted_call: &str,
    expected_result_count: usize,
) {
    let messages = body["messages"]
        .as_array()
        .expect("actual Anthropic request messages");
    let assistant = messages
        .iter()
        .find(|message| {
            message["role"] == "assistant"
                && message["content"].as_array().is_some_and(|blocks| {
                    let calls: Vec<_> = blocks
                        .iter()
                        .filter(|block| block["type"] == "tool_use")
                        .collect();
                    calls.len() == 2
                        && calls[0]["id"] == denied_call
                        && calls[1]["id"] == permitted_call
                })
        })
        .expect("one actual assistant message owns both sibling calls");
    let blocks = assistant["content"].as_array().unwrap();
    let calls: Vec<_> = blocks
        .iter()
        .filter(|block| block["type"] == "tool_use")
        .collect();
    assert_eq!(
        (calls[0]["id"].as_str(), calls[0]["name"].as_str()),
        (Some(denied_call), Some("delete_record"))
    );
    assert_eq!(
        (calls[1]["id"].as_str(), calls[1]["name"].as_str()),
        (Some(permitted_call), Some("read_record"))
    );
    let results: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_array())
        .flat_map(|blocks| blocks.iter())
        .filter(|block| block["type"] == "tool_result")
        .collect();
    assert_eq!(
        results.len(),
        expected_result_count,
        "exact committed result count"
    );
    let results: Vec<_> = results
        .into_iter()
        .filter(|result| {
            result["tool_use_id"] == denied_call || result["tool_use_id"] == permitted_call
        })
        .collect();
    assert_eq!(
        results.len(),
        2,
        "exactly one result for each current sibling"
    );
    let refused = results
        .iter()
        .find(|result| result["tool_use_id"] == denied_call)
        .unwrap();
    let allowed = results
        .iter()
        .find(|result| result["tool_use_id"] == permitted_call)
        .unwrap();
    assert_eq!(refused["is_error"], true);
    let refused_text = wire_text(&refused["content"]);
    let diagnostic: Value = serde_json::from_str(&refused_text).expect("typed safe refusal JSON");
    assert_eq!(
        diagnostic,
        ToolError::AuthorizationRefused { refusal: denied() }.to_error_payload()
    );
    assert!(!refused_text.contains("synthetic-e1"));
    assert_ne!(allowed["is_error"], true);
    assert_eq!(wire_text(&allowed["content"]), "record-7 value");
}
fn wire_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.into();
    }
    content
        .as_array()
        .expect("text or text-block content")
        .iter()
        .map(|block| block["text"].as_str().expect("text block"))
        .collect()
}

async fn exercise(server: &Server) {
    let client = http_client(server);
    let selected = client
        .controller_model_selection()
        .expect("actual registry selection");
    let endpoint = format!("{}/v1/messages", server.base_url);
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("e1-native-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .unwrap(),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .unwrap();
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .unwrap();
    let expected_controller = controller.clone();
    let expected_operation = operation.clone();
    let expected_selection = selected.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        let candidate = claimed.candidate();
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("native-loop").unwrap()
            || candidate.logical_executor != principal("executor")
            || candidate.represented_subject.is_some()
            || candidate.target.logical_runtime != id(&runtime.to_string())
            || candidate.controller_grant_lineage != [expected_controller.clone()]
            || candidate.authority_basis
                != (WorkAuthorityBasis::GrantLineage {
                    lineage: vec![expected_operation.clone()],
                })
            || candidate.controller_model.as_ref() != Some(&expected_selection)
            || candidate.controller_ceiling != ceiling("infer")
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                grants,
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(HttpRecordOwner {
                    selection: selected.clone(),
                    endpoint: endpoint.clone(),
                }),
            })
            .unwrap(),
    );
    let tools = Arc::new(RecordingTools::default());
    let sessions = Arc::new(RecordingStore::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(client);
    builder.default_tool_dispatcher = Some(tools.clone());
    builder.default_session_store = Some(sessions.clone());
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: E1_MODEL.into(),
            prompt: "".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(meerkat_core::service::SessionBuildOptions {
                auth_binding: Some(
                    selected
                        .auth_binding()
                        .expect("registered fixture binding")
                        .clone(),
                ),
                ..Default::default()
            }),
            labels: None,
        },
        false,
    )
    .await
    .unwrap();
    let session_id = created.session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .unwrap();
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .unwrap();
    assert!(
        pin.selection() == &selected,
        "actual actor retained the registry-selected HTTP client"
    );
    let vault = Arc::new(meerkat_auth_core::EphemeralTokenStore::new());
    meerkat_auth_core::save_tokens_and_publish_lifecycle(
        meerkat_core::auth::ProviderAuthPersistence::new(
            vault,
            Arc::new(meerkat_auth_core::InMemoryCoordinator::new()),
        ),
        machine.generated_auth_lease_handle(),
        selected.credential().clone(),
        meerkat_core::auth::PersistedTokens::api_key("synthetic-e1-loopback-only"),
    )
    .await
    .unwrap();
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let claims = association(&runtime, controller, operation, selected.clone());
    let mut prompt = PromptInput::new("Attempt the two record actions", None);
    prompt.header.authority_association = Some(claims.clone());
    let input = Input::Prompt(prompt);
    let input_id = input.id().clone();
    let evidence = Evidence::start(&session_id, &input_id);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::evidence("e1-current-authentication"),
    )
    .unwrap()
    .with_controller_client(&input, pin)
    .unwrap();
    let input = input.with_ingress_context(current).unwrap();
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .unwrap();
    let completion = completion.expect("actual native completion receipt");
    tokio::time::timeout(
        Duration::from_secs(20),
        server.receiver.second_request.notified(),
    )
    .await
    .expect("same native run sends sibling feedback in request two");
    let bodies = server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2);
    for body in &bodies {
        assert_eq!(body["model"], E1_MODEL, "actual provider wire model");
        assert_eq!(body["stream"], true);
    }
    assert_wire_sibling_feedback(&bodies[1]);
    assert_eq!(
        *tools.0.lock().unwrap(),
        ["read_record"],
        "real body oracle excludes delete and preserves read"
    );
    let stored = machine
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .expect("actual unfinished native row");
    let audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
        serde_json::to_value(&stored).unwrap()["authorization_audit"].clone(),
    )
    .unwrap();
    let run_id = audit.first().unwrap().observation.run_id.clone().unwrap();
    for record in &audit {
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(record.contributors[0].input_id, input_id);
        assert!(record.contributors[0].requester == principal("requester"));
        assert!(record.contributors[0].logical_executor == principal("executor"));
        assert!(record.contributors[0].represented_subject.is_none());
        assert_eq!(record.observation.run_id.as_ref(), Some(&run_id));
        assert!(
            matches!(&record.observation.execution_scope, OperationExecutionScope::RuntimeInput {
            owner_session_id, submitted_input_id, canonical_input_id, ..
        } if owner_session_id == &session_id && submitted_input_id == &input_id && canonical_input_id == &input_id)
        );
    }
    let refused = audit.iter().find(|record| matches!(&record.observation.observation,
        AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. } if call_id == DENIED_CALL && tool_name == "delete_record"))).unwrap();
    let allowed = audit.iter().find(|record| matches!(&record.observation.observation,
        AuditObservation::Prepared { target, .. }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. } if call_id == PERMITTED_CALL && tool_name == "read_record"))).unwrap();
    assert!(refused.observation.operation_id != allowed.observation.operation_id);
    assert!(!audit.iter().any(|record| record.observation.operation_id
        == refused.observation.operation_id
        && matches!(
            record.observation.observation,
            AuditObservation::Entry | AuditObservation::Outcome { .. }
        )));
    assert!(audit.iter().any(|record| record.observation.operation_id
        == allowed.observation.operation_id
        && matches!(record.observation.observation, AuditObservation::Entry)));
    assert!(audit.iter().any(|record| record.observation.operation_id
        == allowed.observation.operation_id
        && matches!(
            &record.observation.observation,
            AuditObservation::Outcome {
                outcome: OperationObservedOutcome::ToolDispatchReturned {
                    result_is_error: false,
                    terminal_error: None,
                    ..
                }
            }
        )));
    let model_ids: Vec<_> = audit
        .iter()
        .filter_map(|record| match &record.observation.observation {
            AuditObservation::Prepared { target, .. }
                if matches!(target.as_ref(), AuditTarget::Model(_)) =>
            {
                Some(record.observation.operation_id.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(model_ids.len(), 2);
    for operation_id in &model_ids {
        assert!(
            audit
                .iter()
                .any(|record| &record.observation.operation_id == operation_id
                    && matches!(record.observation.observation, AuditObservation::Entry))
        );
    }
    assert!(
        audit
            .iter()
            .any(|record| record.observation.operation_id == model_ids[0]
                && matches!(
                    record.observation.observation,
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::HttpResponse { status: 200 }
                    }
                ))
    );
    evidence.write("observed.json", &json!({"checkpoint":"E1","status":"observed_before_completion","session_id":session_id,"input_id":input_id,"run_id":run_id,"association":claims,"audit":audit}));
    server.receiver.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("E1 must complete the same native run: {outcome:?}")
    };
    assert_eq!(result.text, FINISHED);
    assert_eq!(result.session_id, session_id);
    assert!(result.terminal_cause_kind.is_none());
    let saved = sessions
        .0
        .lock()
        .unwrap()
        .get(&session_id)
        .cloned()
        .expect("real final session store");
    let mut paired = false;
    for pair in saved.messages().windows(2) {
        if let [
            Message::BlockAssistant(message),
            Message::ToolResults { results, .. },
        ] = pair
        {
            let ids: Vec<_> = message
                .tool_calls()
                .map(|call| call.id.to_string())
                .collect();
            if ids == [DENIED_CALL, PERMITTED_CALL] {
                assert_eq!(results.len(), 2);
                assert!(
                    results
                        .iter()
                        .any(|result| result.tool_use_id == DENIED_CALL && result.is_error)
                );
                assert!(
                    results
                        .iter()
                        .any(|result| result.tool_use_id == PERMITTED_CALL
                            && !result.is_error
                            && result.text_content() == "record-7 value")
                );
                paired = true;
            }
        }
    }
    assert!(
        paired,
        "canonical saved assistant and result batch preserve both siblings"
    );
    let entries = tools.0.lock().unwrap().clone();
    let model_count = server.receiver.bodies.lock().unwrap().len();
    let authorization_count = server.receiver.authorized_requests.load(Ordering::SeqCst);
    assert_eq!(entries, ["read_record"]);
    assert_eq!(model_count, 2);
    assert_eq!(authorization_count, model_count);
    evidence.write("completed.json", &json!({
        "checkpoint":"E1", "status":"completed", "session_id":session_id,"input_id":input_id,"run_id":run_id,
        "selected_controller":selected,"endpoint":endpoint,"association":claims,
        "denied_operation_id":refused.observation.operation_id,"allowed_operation_id":allowed.observation.operation_id,
        "model_operation_ids":model_ids,"model_http_count":model_count,"authorization_count":authorization_count,
        "body_entries":entries,"delete_count":entries.iter().filter(|entry| entry.as_str() == "delete_record").count(),
        "read_count":entries.iter().filter(|entry| entry.as_str() == "read_record").count(),
        "result":{"text":result.text,"terminal_cause_kind":result.terminal_cause_kind},
        "audit_capture_phase":"second_http_request_waiting_for_response","audit":audit,
    }));
}

#[path = "e1_policy_control/a8_requester_revocation.rs"]
mod a8_requester_revocation;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "E1 acceptance needs explicit evidence output and native composed authorization"]
async fn adr_e1_policy_control_same_batch_native_run() {
    let mut server = Server::start().await;
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        exercise(&server),
    ))
    .catch_unwind()
    .await;
    server.reap().await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("E1 timed out: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[path = "e1_policy_control/a2_resource_specificity.rs"]
mod a2_resource_specificity;

#[path = "e1_policy_control/a3_queued_work.rs"]
mod a3_queued_work;

#[path = "e1_policy_control/stock_persistent.rs"]
mod stock_persistent;

#[cfg(all(target_os = "macos", feature = "integration-real-tests"))]
#[path = "e1_policy_control/shell_confinement.rs"]
mod shell_confinement;
