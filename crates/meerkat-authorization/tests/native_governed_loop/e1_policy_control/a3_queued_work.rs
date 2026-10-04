//! A3: queued native work survives a local refusal in the preceding run.
//! Private HTTP scripting is deterministic; no live-model or persistence claim.
use super::*;
use meerkat_core::HandlingMode;
use meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata;
use meerkat_runtime::accept::AcceptOutcome;

const QUEUED_CALL: &str = "a3-queued-read";
const QUEUED_PROMPT: &str = "A3 follow-up allowed read";
const QUEUED_FINISHED: &str = "queued native read completed";

#[derive(Default)]
struct QueueBarriers {
    first_read: Notify,
    release_read: Notify,
    queued_feedback: Notify,
    release_queued: Notify,
}

// Unblock every fixture wait if an assertion/timeout unwinds the exercise.
// These are receiver controls, never authorization or native lifecycle state.
struct ReleaseOnDrop {
    barriers: Arc<QueueBarriers>,
    receiver: Arc<Receiver>,
}
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.barriers.release_read.notify_one();
        self.barriers.release_queued.notify_one();
        self.receiver.finish.notify_one();
    }
}

struct QueueTools {
    inner: RecordingTools,
    barriers: Arc<QueueBarriers>,
    calls: Mutex<Vec<(String, String)>>,
}
#[async_trait]
impl AgentToolDispatcher for QueueTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("A3 must reach the actual governed context at the receiver")
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        assert!(context.work_authorization().is_some());
        self.calls
            .lock()
            .unwrap()
            .push((call.id.to_owned(), call.name.to_owned()));
        if call.id == PERMITTED_CALL {
            self.barriers.first_read.notify_one();
            self.barriers.release_read.notified().await;
        }
        self.inner.dispatch_with_context(call, context).await
    }
}

fn queued_read_response() -> String {
    sse(vec![
        start_message(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":QUEUED_CALL,"name":"read_record","input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"record\":\"record-7\"}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","usage":{"output_tokens":2},"delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ])
}
fn queued_final_response() -> String {
    sse(vec![
        start_message(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":QUEUED_FINISHED}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","usage":{"output_tokens":3},"delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_stop"}),
    ])
}
async fn receive_queued(
    State((receiver, barriers)): State<(Arc<Receiver>, Arc<QueueBarriers>)>,
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
    let response = match count {
        1 => sibling_response(),
        2 => {
            receiver.second_request.notify_one();
            receiver.finish.notified().await;
            final_response()
        }
        3 => queued_read_response(),
        4 => {
            barriers.queued_feedback.notify_one();
            barriers.release_queued.notified().await;
            queued_final_response()
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                [("content-type", "application/json")],
                "unexpected extra A3 request".into(),
            );
        }
    };
    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        response,
    )
}
// Inspect only actual user text blocks, not assistant/tool output or a whole
// JSON serialization that could confuse a tool argument with delivered input.
fn user_text_marker_count(body: &Value, marker: &str) -> usize {
    body["messages"]
        .as_array()
        .expect("actual Anthropic messages array")
        .iter()
        .filter(|message| message["role"] == "user")
        .map(|message| {
            if let Some(text) = message["content"].as_str() {
                text.matches(marker).count()
            } else {
                message["content"]
                    .as_array()
                    .expect("Anthropic user content is text or content blocks")
                    .iter()
                    .filter(|block| block["type"] == "text")
                    .map(|block| {
                        block["text"]
                            .as_str()
                            .expect("Anthropic text block has string text")
                            .matches(marker)
                            .count()
                    })
                    .sum()
            }
        })
        .sum()
}

async fn start_server(barriers: Arc<QueueBarriers>) -> Server {
    let receiver = Arc::new(Receiver::default());
    let app = Router::new()
        .route("/v1/messages", post(receive_queued))
        .with_state((receiver.clone(), barriers));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server {
        base_url,
        receiver,
        task: Some(task),
    }
}
fn queue_input(
    claims: &InputAuthorityAssociation,
    label: &str,
    prompt: &str,
    pin: meerkat_core::ControllerModelClient,
) -> Input {
    let mut candidate = claims.candidate().clone();
    candidate.original_work.work = id(label);
    candidate.ingress_namespace.occurrence_scope = id(label);
    candidate.root_event = super::super::evidence(label);
    let mut input = PromptInput::new(
        prompt,
        Some(RuntimeTurnMetadata {
            handling_mode: Some(HandlingMode::Queue),
            ..Default::default()
        }),
    );
    input.header.authority_association = Some(InputAuthorityAssociation::new(candidate).unwrap());
    let input = Input::Prompt(input);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::super::evidence(label),
    )
    .unwrap()
    .with_controller_client(&input, pin)
    .unwrap();
    input.with_ingress_context(current).unwrap()
}
fn observations(
    stored: &meerkat_runtime::input_state::StoredInputState,
) -> Vec<StoredAuthorizationAuditObservation> {
    serde_json::from_value(
        serde_json::to_value(stored)
            .unwrap()
            .get("authorization_audit")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .unwrap()
}
fn assert_originals(
    audit: &[StoredAuthorizationAuditObservation],
    session_id: &SessionId,
    input_id: &meerkat_core::InputId,
    run_id: &meerkat_core::RunId,
) {
    assert!(!audit.is_empty());
    for record in audit {
        assert_eq!(record.observation.run_id.as_ref(), Some(run_id));
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(record.contributors[0].input_id, *input_id);
        assert_eq!(record.contributors[0].requester, principal("requester"));
        assert_eq!(
            record.contributors[0].logical_executor,
            principal("executor")
        );
        assert!(record.contributors[0].represented_subject.is_none());
        assert!(
            matches!(&record.observation.execution_scope, OperationExecutionScope::RuntimeInput {
            owner_session_id, submitted_input_id, canonical_input_id, ..
        } if owner_session_id == session_id && submitted_input_id == input_id && canonical_input_id == input_id)
        );
    }
}
fn assert_tool_returned(
    audit: &[StoredAuthorizationAuditObservation],
    call: &str,
) -> meerkat_core::OperationId {
    let prepared: Vec<_> = audit.iter().filter(|record| matches!(&record.observation.observation,
        AuditObservation::Prepared { target, .. } if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
            if call_id == call && tool_name == "read_record"))).collect();
    assert_eq!(prepared.len(), 1);
    let operation = prepared[0].observation.operation_id.clone();
    assert!(
        audit
            .iter()
            .any(|record| record.observation.operation_id == operation
                && matches!(record.observation.observation, AuditObservation::Entry))
    );
    assert!(
        audit
            .iter()
            .any(|record| record.observation.operation_id == operation
                && matches!(
                    record.observation.observation,
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::ToolDispatchReturned {
                            result_is_error: false,
                            terminal_error: None,
                            ..
                        }
                    }
                ))
    );
    operation
}
// Config::default enables the model's supported hosted web search. This
// fixture denies that capability, so each new run must refuse it before entry
// and continue once with the same bare controller. No other refusal is allowed.
fn assert_queued_controller_fallback(
    audit: &[StoredAuthorizationAuditObservation],
    selected: &ControllerModelSelection,
    endpoint: &str,
    requests: &[Value],
) -> meerkat_core::OperationId {
    use meerkat_authorization_contracts::audit::{AuditModelTarget, AuditModelUse};

    let diagnostic = serde_json::to_value(audit).unwrap();
    let bare = AuditModelTarget {
        model: selected.model().to_owned(),
        provider: selected.provider(),
        self_hosted_server_id: selected.self_hosted_server_id().map(str::to_owned),
        auth_binding: selected.auth_binding().cloned(),
        credential: Some(selected.credential().clone()),
        wire_model: E1_MODEL.to_owned(),
        backend_profile_id: Some(selected.backend_profile_id().to_owned()),
        backend_kind: selected.backend_kind().to_owned(),
        endpoint: endpoint.to_owned(),
        hosted_capabilities: Vec::new(),
        usage: AuditModelUse::ControllerInference,
    };
    let mut hosted = bare.clone();
    hosted.hosted_capabilities = vec![meerkat_core::ServerToolKind::WebSearch];
    let refusals: Vec<_> = audit
        .iter()
        .enumerate()
        .filter(|(_, record)| {
            matches!(
                record.observation.observation,
                AuditObservation::Refused { .. }
            )
        })
        .collect();
    assert_eq!(
        refusals.len(),
        1,
        "queued run permits only its one hosted-default refusal: {diagnostic}"
    );
    let (refused_index, refused) = refusals[0];
    assert!(
        matches!(&refused.observation.observation,
            AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
            if target.as_ref() == &AuditTarget::Model(hosted)
        ),
        "unexpected queued refusal target or reason: {diagnostic}"
    );
    let refused_id = &refused.observation.operation_id;
    assert!(
        !audit
            .iter()
            .any(|record| &record.observation.operation_id == refused_id
                && matches!(
                    record.observation.observation,
                    AuditObservation::Prepared { .. }
                        | AuditObservation::Entry
                        | AuditObservation::Outcome { .. }
                )),
        "refused hosted request must not prepare or enter: {diagnostic}"
    );
    let models: Vec<_> = audit.iter().enumerate().filter(|(_, record)| matches!(
        &record.observation.observation,
        AuditObservation::Prepared { target, .. } if matches!(target.as_ref(), AuditTarget::Model(_))
    )).collect();
    assert_eq!(
        models.len(),
        2,
        "queued run must prepare exactly two actual model requests: {diagnostic}"
    );
    assert!(
        models[0].1.observation.operation_id != models[1].1.observation.operation_id,
        "each actual model request owns its operation: {diagnostic}"
    );
    for (index, record) in &models {
        assert!(
            *index > refused_index,
            "bare controller preparation follows hosted refusal: {diagnostic}"
        );
        assert!(
            matches!(&record.observation.observation,
                AuditObservation::Prepared { target, .. } if target.as_ref() == &AuditTarget::Model(bare.clone())
            ),
            "queued request must preserve the exact controller and remove hosted capability: {diagnostic}"
        );
        assert!(
            audit.iter().any(|entry| entry.observation.operation_id
                == record.observation.operation_id
                && matches!(entry.observation.observation, AuditObservation::Entry)),
            "each actual model request has entry evidence: {diagnostic}"
        );
    }
    assert!(
        audit.iter().any(|record| record.observation.operation_id
            == models[0].1.observation.operation_id
            && matches!(
                record.observation.observation,
                AuditObservation::Outcome {
                    outcome: OperationObservedOutcome::HttpResponse { status: 200 }
                }
            )),
        "first queued model response must complete before tool feedback: {diagnostic}"
    );
    assert_eq!(requests.len(), 2);
    for request in requests {
        assert!(
            !request["tools"].as_array().is_some_and(|tools| tools
                .iter()
                .any(|tool| tool["name"] == "web_search" || tool["type"] == "web_search_20250305")),
            "refused hosted capability must not reach either physical HTTP request: {request}"
        );
    }
    refused_id.clone()
}
fn start_evidence(session: &SessionId) -> Evidence {
    let root = ordinary_evidence_root("MEERKAT_A3_EVIDENCE_DIR");
    let directory = root.join(format!("adr-a3-{session}"));
    std::fs::create_dir_all(directory.parent().unwrap()).unwrap();
    std::fs::create_dir(&directory).unwrap();
    Evidence { directory }
}

async fn exercise_queued(server: &Server, barriers: Arc<QueueBarriers>) {
    let _release = ReleaseOnDrop {
        barriers: barriers.clone(),
        receiver: server.receiver.clone(),
    };
    let client = http_client(server);
    let selected = client
        .controller_model_selection()
        .expect("actual registry selection");
    let endpoint = format!("{}/v1/messages", server.base_url);
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("a3-native-grants"),
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
    let tools = Arc::new(QueueTools {
        inner: RecordingTools::default(),
        barriers: barriers.clone(),
        calls: Mutex::new(Vec::new()),
    });
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
                auth_binding: selected.auth_binding().cloned(),
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
    let first = queue_input(
        &claims,
        "a3-first",
        "Attempt the two record actions",
        pin.clone(),
    );
    let first_id = first.id().clone();
    let first_claims = first.header().authority_association.clone().unwrap();
    let queued = queue_input(&claims, "a3-queued", QUEUED_PROMPT, pin);
    let queued_id = queued.id().clone();
    let queued_claims = queued.header().authority_association.clone().unwrap();
    assert_ne!(first_id, queued_id);
    let evidence = start_evidence(&session_id);
    evidence.write("started.json", &json!({"checkpoint":"A3","session_id":session_id,"first_input_id":first_id,"queued_input_id":queued_id}));
    let (accepted, first_completion) = machine
        .accept_input_with_completion(&session_id, first)
        .await
        .unwrap();
    assert!(
        matches!(accepted, AcceptOutcome::Accepted { ref input_id, .. } if input_id == &first_id)
    );
    let first_completion = first_completion.expect("first actual native receipt");
    tokio::time::timeout(Duration::from_secs(20), barriers.first_read.notified())
        .await
        .expect("first real read reached receiver barrier");

    let (accepted, queued_completion) = machine
        .accept_input_with_completion(&session_id, queued)
        .await
        .unwrap();
    let AcceptOutcome::Accepted {
        input_id: accepted_id,
        seed,
        ..
    } = accepted
    else {
        panic!("distinct queue input must be newly accepted: {accepted:?}")
    };
    assert_eq!(accepted_id, queued_id);
    assert!(
        seed.last_run_id.is_none(),
        "queued admission cannot join the first run"
    );
    let queued_completion = queued_completion.expect("queued actual native receipt");
    let queued_before = machine
        .input_state(&session_id, &queued_id)
        .await
        .unwrap()
        .expect("real queued row");
    assert!(queued_before.seed.last_run_id.is_none());
    assert!(
        observations(&queued_before).is_empty(),
        "queued work has not prepared any operation"
    );
    assert_eq!(queued_before.state.authority_contributors.len(), 1);
    assert_eq!(
        queued_before.state.authority_contributors[0].input_id(),
        &queued_id
    );
    assert_eq!(
        queued_before.state.authority_contributors[0]
            .association()
            .candidate()
            .original_work
            .work,
        id("a3-queued")
    );
    assert_eq!(
        server.receiver.bodies.lock().unwrap().len(),
        1,
        "no queued or continuation HTTP while first read is held"
    );
    assert_eq!(
        *tools.calls.lock().unwrap(),
        [(PERMITTED_CALL.to_owned(), "read_record".to_owned())]
    );
    assert!(
        tools.inner.0.lock().unwrap().is_empty(),
        "first body has entered its receiver but not finished"
    );
    evidence.write("queued.json", &json!({"checkpoint":"A3","queued_input_id":queued_id,"queued_run_id":queued_before.seed.last_run_id,"audit_count":0,"http_count":1}));

    barriers.release_read.notify_one();
    tokio::time::timeout(
        Duration::from_secs(20),
        server.receiver.second_request.notified(),
    )
    .await
    .expect("first run returned both sibling results to controller");
    let first_bodies = server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(first_bodies.len(), 2);
    assert_wire_sibling_feedback(&first_bodies[1]);
    let first_row = machine
        .input_state(&session_id, &first_id)
        .await
        .unwrap()
        .unwrap();
    let first_run = first_row
        .seed
        .last_run_id
        .clone()
        .expect("first actual generated RunId");
    let first_audit = observations(&first_row);
    assert_originals(&first_audit, &session_id, &first_id, &first_run);
    let denied: Vec<_> = first_audit.iter().filter(|record| matches!(&record.observation.observation,
        AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. } if call_id == DENIED_CALL && tool_name == "delete_record"))).collect();
    assert_eq!(denied.len(), 1);
    assert!(
        !first_audit
            .iter()
            .any(
                |record| record.observation.operation_id == denied[0].observation.operation_id
                    && matches!(
                        record.observation.observation,
                        AuditObservation::Entry | AuditObservation::Outcome { .. }
                    )
            )
    );
    let first_read = assert_tool_returned(&first_audit, PERMITTED_CALL);
    server.receiver.finish.notify_one();
    let first_outcome = tokio::time::timeout(Duration::from_secs(20), first_completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(first_result) = first_outcome else {
        panic!("first run failed after local refusal: {first_outcome:?}")
    };
    assert_eq!(first_result.text, FINISHED);
    assert_eq!(first_result.session_id, session_id);
    assert!(first_result.terminal_cause_kind.is_none());

    tokio::time::timeout(Duration::from_secs(20), barriers.queued_feedback.notified())
        .await
        .expect("queued run executed its read and sent controller feedback");
    let bodies = server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 4);
    for body in &bodies[..2] {
        assert_eq!(
            user_text_marker_count(body, QUEUED_PROMPT),
            0,
            "the queued prompt must not enter either first-run request"
        );
    }
    assert_eq!(
        user_text_marker_count(&bodies[2], QUEUED_PROMPT),
        1,
        "the third HTTP request must deliver the distinct queued user prompt"
    );
    for body in &bodies {
        assert_eq!(body["model"], E1_MODEL);
        assert_eq!(body["stream"], true);
    }
    let queued_results: Vec<_> = bodies[3]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_array())
        .flat_map(|content| content.iter())
        .filter(|block| block["type"] == "tool_result" && block["tool_use_id"] == QUEUED_CALL)
        .collect();
    assert_eq!(queued_results.len(), 1);
    assert_ne!(queued_results[0]["is_error"], true);
    assert_eq!(wire_text(&queued_results[0]["content"]), "record-7 value");
    let queued_row = machine
        .input_state(&session_id, &queued_id)
        .await
        .unwrap()
        .unwrap();
    let queued_run = queued_row
        .seed
        .last_run_id
        .clone()
        .expect("queued actual generated RunId");
    assert_ne!(first_run, queued_run);
    let queued_audit = observations(&queued_row);
    assert_originals(&queued_audit, &session_id, &queued_id, &queued_run);
    // Retain actual records before the oracle, including unexpected failures.
    evidence.write(
        "queued-observed-before-completion.json",
        &json!({
            "checkpoint":"A3","session_id":session_id,"input_id":queued_id,
            "run_id":queued_run,"http_count":bodies.len(),"audit":queued_audit,
        }),
    );
    let queued_hosted_refusal =
        assert_queued_controller_fallback(&queued_audit, &selected, &endpoint, &bodies[2..]);
    let queued_read = assert_tool_returned(&queued_audit, QUEUED_CALL);
    assert_ne!(first_read, queued_read);
    assert_eq!(
        *tools.calls.lock().unwrap(),
        [
            (PERMITTED_CALL.to_owned(), "read_record".to_owned()),
            (QUEUED_CALL.to_owned(), "read_record".to_owned()),
        ],
        "exact receiver IDs prove one read in each run and zero delete entry"
    );
    barriers.release_queued.notify_one();
    let queued_outcome = tokio::time::timeout(Duration::from_secs(20), queued_completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(queued_result) = queued_outcome else {
        panic!("queued run inherited fatal state: {queued_outcome:?}")
    };
    assert_eq!(queued_result.text, QUEUED_FINISHED);
    assert_eq!(queued_result.session_id, session_id);
    assert!(queued_result.terminal_cause_kind.is_none());
    // Both completion handles are resolved before final exact row/run checks.
    let first_done = machine
        .input_state(&session_id, &first_id)
        .await
        .unwrap()
        .unwrap();
    let queued_done = machine
        .input_state(&session_id, &queued_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_done.state.authority_contributors.len(), 1);
    assert_eq!(queued_done.state.authority_contributors.len(), 1);
    assert!(first_done.state.authority_contributors[0].association() == &first_claims);
    assert!(queued_done.state.authority_contributors[0].association() == &queued_claims);
    assert_eq!(first_done.seed.last_run_id.as_ref(), Some(&first_run));
    assert_eq!(queued_done.seed.last_run_id.as_ref(), Some(&queued_run));
    assert_originals(
        &observations(&first_done),
        &session_id,
        &first_id,
        &first_run,
    );
    assert_originals(
        &observations(&queued_done),
        &session_id,
        &queued_id,
        &queued_run,
    );
    assert_eq!(
        *tools.inner.0.lock().unwrap(),
        ["read_record", "read_record"]
    );
    assert_eq!(server.receiver.bodies.lock().unwrap().len(), 4);
    assert_eq!(
        server.receiver.authorized_requests.load(Ordering::SeqCst),
        4
    );
    let saved = sessions
        .0
        .lock()
        .unwrap()
        .get(&session_id)
        .cloned()
        .unwrap();
    for (call, error) in [
        (DENIED_CALL, true),
        (PERMITTED_CALL, false),
        (QUEUED_CALL, false),
    ] {
        let results: Vec<_> = saved
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::ToolResults { results, .. } => Some(results),
                _ => None,
            })
            .flat_map(|results| results.iter())
            .filter(|result| result.tool_use_id == call)
            .collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].is_error, error);
    }
    evidence.write("completed.json", &json!({
        "checkpoint":"A3","status":"completed","session_id":session_id,
        "first_input_id":first_id,"queued_input_id":queued_id,"first_run_id":first_run,"queued_run_id":queued_run,
        "first_read_operation_id":first_read,"queued_read_operation_id":queued_read,
        "queued_hosted_refusal_operation_id":queued_hosted_refusal,
        "first_audit":observations(&first_done),"queued_audit":observations(&queued_done),
        "body_calls":*tools.calls.lock().unwrap(),"http_count":4,"authorization_count":4,
        "first_terminal_cause":first_result.terminal_cause_kind,"queued_terminal_cause":queued_result.terminal_cause_kind,
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adr_a3_queued_work_survives_local_permission_refusal() {
    let barriers = Arc::new(QueueBarriers::default());
    let mut server = start_server(barriers.clone()).await;
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        exercise_queued(&server, barriers),
    ))
    .catch_unwind()
    .await;
    tokio::time::timeout(Duration::from_secs(5), server.reap())
        .await
        .expect("recording server cleanup is bounded");
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("A3 timed out: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
