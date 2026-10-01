//! A2 application resource-owner integration through the actual HTTP/native loop.
//! Exact resource ACLs belong to this configured application owner. Generated
//! grants retain ordinary action/domain ceilings; no generic ABAC engine is claimed.
use super::*;
use meerkat_core::RunId;
use std::collections::{BTreeMap, BTreeSet};

const DELETE_T: &str = "a2-delete-t";
const READ_T: &str = "a2-read-t";
const DELETE_T2: &str = "a2-delete-t2";
const TARGET_T: &str = "record-7";
const TARGET_T2: &str = "record-8";
const READ_RESULT: &str = "record-7 original value";
const DELETE_RESULT: &str = "record-8 deleted";

#[derive(Clone, Copy)]
enum Order {
    DeniedFirst,
    DeniedLast,
}
impl Order {
    fn name(self) -> &'static str {
        match self {
            Self::DeniedFirst => "denied-first",
            Self::DeniedLast => "denied-last",
        }
    }
    fn calls(self) -> [(&'static str, &'static str, &'static str); 3] {
        let denied = (DELETE_T, "delete_record", TARGET_T);
        let read = (READ_T, "read_record", TARGET_T);
        let allowed = (DELETE_T2, "delete_record", TARGET_T2);
        match self {
            Self::DeniedFirst => [denied, read, allowed],
            Self::DeniedLast => [allowed, read, denied],
        }
    }
}

fn sibling_response_a2(order: Order) -> String {
    let mut events = vec![start_message()];
    for (index, (id, name, record)) in order.calls().into_iter().enumerate() {
        events.extend([
            json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
            json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":json!({"record":record}).to_string()}}),
            json!({"type":"content_block_stop","index":index}),
        ]);
    }
    events.extend([
        json!({"type":"message_delta","usage":{"output_tokens":3},"delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ]);
    sse(events)
}

async fn receive_a2(
    State((receiver, order)): State<(Arc<Receiver>, Order)>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    // The provider's actual received JSON is recorded verbatim, never rewritten.
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
    match count {
        1 => (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            sibling_response_a2(order),
        ),
        2 => {
            receiver.second_request.notify_one();
            receiver.finish.notified().await;
            (
                StatusCode::OK,
                [("content-type", "text/event-stream")],
                final_response(),
            )
        }
        _ => (
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json")],
            "unexpected extra request".into(),
        ),
    }
}

async fn start_server(order: Order) -> Server {
    let receiver = Arc::new(Receiver::default());
    let app = Router::new()
        .route("/v1/messages", post(receive_a2))
        .with_state((receiver.clone(), order));
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

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ResourcePermission {
    principal: PrincipalRef,
    action: ActionRef,
    resource: ResourceRef,
}
struct Record {
    target: ResourceRef,
    value: String,
    deleted: bool,
}
#[derive(Clone, serde::Serialize)]
struct BodyEntry {
    call_id: String,
    tool: String,
    target: ResourceRef,
    run_id: RunId,
}
#[derive(Clone, serde::Serialize)]
struct PolicyRead {
    call_id: String,
    requester: PrincipalRef,
    action: ActionRef,
    target: ResourceRef,
}

/// One canonical application catalog and ACL table, configured before native
/// exposure and immutable for the scenario. Both policy and physical effects
/// resolve the same catalog. No per-target namespace or permission boolean exists.
struct RecordApplication {
    records: Mutex<BTreeMap<String, Record>>,
    permissions: BTreeSet<ResourcePermission>,
    actions: BTreeMap<String, ActionRef>,
    entries: Mutex<Vec<BodyEntry>>,
    policy_reads: Mutex<Vec<PolicyRead>>,
}
impl RecordApplication {
    fn configured() -> Self {
        let target = |id: &str| ResourceRef {
            domain: domain(),
            resource_id: id.into(),
        };
        let records = [
            (
                TARGET_T.into(),
                Record {
                    target: target(TARGET_T),
                    value: READ_RESULT.into(),
                    deleted: false,
                },
            ),
            (
                TARGET_T2.into(),
                Record {
                    target: target(TARGET_T2),
                    value: "record-8 original value".into(),
                    deleted: false,
                },
            ),
        ]
        .into_iter()
        .collect();
        let permissions = [
            ResourcePermission {
                principal: principal("requester"),
                action: action("read"),
                resource: target(TARGET_T),
            },
            ResourcePermission {
                principal: principal("requester"),
                action: action("delete"),
                resource: target(TARGET_T2),
            },
        ]
        .into_iter()
        .collect();
        Self {
            records: Mutex::new(records),
            permissions,
            actions: [
                ("read_record".into(), action("read")),
                ("delete_record".into(), action("delete")),
            ]
            .into_iter()
            .collect(),
            entries: Mutex::new(Vec::new()),
            policy_reads: Mutex::new(Vec::new()),
        }
    }
    fn resolve(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<(ActionRef, ResourceRef), meerkat_core::OperationAuthorizationError> {
        let arguments: RecordArguments = serde_json::from_str(arguments).map_err(|_| denied())?;
        let action = self.actions.get(name).ok_or_else(denied)?.clone();
        let records = self.records.lock().unwrap();
        let record = records.get(&arguments.record).ok_or_else(denied)?;
        // Permission concerns the catalog identity even after an allowed delete.
        // An immutable target is not inferred from the argument's spelling.
        Ok((action, record.target.clone()))
    }
    fn assert_effects(&self, run_id: &RunId) -> Vec<BodyEntry> {
        let entries = self.entries.lock().unwrap().clone();
        assert_eq!(entries.len(), 2, "exactly two real dispatcher bodies enter");
        assert_eq!(
            entries
                .iter()
                .filter(
                    |entry| entry.tool == "delete_record" && entry.target.resource_id == TARGET_T
                )
                .count(),
            0
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.call_id == READ_T
                    && entry.tool == "read_record"
                    && entry.target.resource_id == TARGET_T)
                .count(),
            1
        );
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.call_id == DELETE_T2
                    && entry.tool == "delete_record"
                    && entry.target.resource_id == TARGET_T2)
                .count(),
            1
        );
        assert!(
            entries
                .iter()
                .all(|entry| &entry.run_id == run_id && entry.target.domain == domain())
        );
        let records = self.records.lock().unwrap();
        assert!(
            !records[TARGET_T].deleted,
            "forbidden record remains physically intact"
        );
        assert_eq!(records[TARGET_T].value, READ_RESULT);
        assert!(
            records[TARGET_T2].deleted,
            "allowed sibling deletes its actual record"
        );
        assert_eq!(
            records[TARGET_T].target.domain,
            records[TARGET_T2].target.domain
        );
        entries
    }
}

struct ResourcePolicy {
    controller: HttpRecordOwner,
    application: Arc<RecordApplication>,
}
impl OperationPolicyOwner for ResourcePolicy {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        now_ms: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        self.controller
            .authorize_controller_admission(association, facts, now_ms)
    }
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let AuthorizationOperation::Tool(facts) = &binding.facts().operation else {
            return self
                .controller
                .authorize_operation(association, binding, purpose, now_ms);
        };
        if purpose != LocalPolicyPurpose::Operation
            || !matches!(facts.target, ToolAuthorizationTarget::Dispatcher(_))
        {
            return Err(denied().into());
        }
        let (operation_action, target) = self
            .application
            .resolve(facts.name.as_str(), facts.arguments.get())?;
        let requester = association.candidate().requester.clone();
        self.application
            .policy_reads
            .lock()
            .unwrap()
            .push(PolicyRead {
                call_id: facts.call_id.to_string(),
                requester: requester.clone(),
                action: operation_action.clone(),
                target: target.clone(),
            });
        if !self.application.permissions.contains(&ResourcePermission {
            principal: requester,
            action: operation_action.clone(),
            resource: target.clone(),
        }) {
            return Err(denied().into());
        }
        Ok(LocalPolicyAllowance {
            operation_values: vec![LocalOperationValues {
                action: operation_action,
                resource_domain: target.domain,
                processor: ProcessorRef::Principal {
                    principal: association.candidate().logical_executor.clone(),
                },
                audience: AudienceRef::Principal {
                    principal: association.candidate().requester.clone(),
                },
            }],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }
}

#[async_trait]
impl AgentToolDispatcher for RecordApplication {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        ["delete_record", "read_record"]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    "fixture record operation",
                    json!({
                        "type":"object", "properties":{"record":{"type":"string"}},
                        "required":["record"], "additionalProperties":false,
                    }),
                ))
            })
            .collect()
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("A2 body requires the actual native dispatch context")
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        assert!(context.work_authorization().is_some());
        let run_id = context.run_id().expect("native tool RunId").clone();
        let arguments: RecordArguments = serde_json::from_str(call.args.get()).unwrap();
        let mut records = self.records.lock().unwrap();
        let record = records
            .get_mut(&arguments.record)
            .expect("actual catalog target");
        self.entries.lock().unwrap().push(BodyEntry {
            call_id: call.id.into(),
            tool: call.name.into(),
            target: record.target.clone(),
            run_id,
        });
        // No permission check here can hide a missed pre-entry authorization.
        // If forbidden delete enters, it really deletes T and fails the oracle.
        let result = match call.name {
            "read_record" => {
                assert!(!record.deleted, "read reached a deleted record");
                record.value.clone()
            }
            "delete_record" => {
                assert!(!record.deleted, "delete physically entered more than once");
                record.deleted = true;
                format!("{} deleted", record.target.resource_id)
            }
            _ => panic!("unknown actual fixture tool"),
        };
        Ok(ToolResult::new(call.id.into(), result, false).into())
    }
}

fn assert_feedback(body: &Value, order: Order) {
    let messages = body["messages"]
        .as_array()
        .expect("actual Anthropic request messages");
    let assistants: Vec<_> = messages
        .iter()
        .filter(|message| {
            message["role"] == "assistant"
                && message["content"]
                    .as_array()
                    .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "tool_use"))
        })
        .collect();
    assert_eq!(
        assistants.len(),
        1,
        "one assistant owns the whole sibling batch"
    );
    let calls: Vec<_> = assistants[0]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|block| block["type"] == "tool_use")
        .collect();
    assert_eq!(calls.len(), 3);
    for (actual, (id, name, record)) in calls.iter().zip(order.calls()) {
        assert_eq!(actual["id"], id);
        assert_eq!(actual["name"], name);
        assert_eq!(actual["input"], json!({"record":record}));
    }
    let results: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_array())
        .flat_map(|blocks| blocks.iter())
        .filter(|block| block["type"] == "tool_result")
        .collect();
    assert_eq!(results.len(), 3);
    for (id, _, _) in order.calls() {
        assert_eq!(
            results
                .iter()
                .filter(|result| result["tool_use_id"] == id)
                .count(),
            1
        );
    }
    let refused = results
        .iter()
        .find(|result| result["tool_use_id"] == DELETE_T)
        .unwrap();
    assert_eq!(refused["is_error"], true);
    let diagnostic: Value = serde_json::from_str(&wire_text(&refused["content"])).unwrap();
    assert_eq!(
        diagnostic,
        ToolError::AuthorizationRefused { refusal: denied() }.to_error_payload()
    );
    for (id, expected) in [(READ_T, READ_RESULT), (DELETE_T2, DELETE_RESULT)] {
        let result = results
            .iter()
            .find(|result| result["tool_use_id"] == id)
            .unwrap();
        assert_ne!(result["is_error"], true);
        assert_eq!(wire_text(&result["content"]), expected);
    }
}

fn evidence_a2(session_id: &SessionId, input_id: &meerkat_core::InputId, order: Order) -> Evidence {
    let root = std::env::var_os("TEST_UNDECLARED_OUTPUTS_DIR")
        .or_else(|| std::env::var_os("MEERKAT_A2_EVIDENCE_DIR"))
        .filter(|path| !path.is_empty())
        .expect("A2 requires an explicit evidence directory");
    let directory = PathBuf::from(root).join(format!("adr-a2-{}-{session_id}", order.name()));
    std::fs::create_dir_all(directory.parent().unwrap()).unwrap();
    std::fs::create_dir(&directory).expect("fresh A2 invocation evidence");
    let evidence = Evidence { directory };
    evidence.write("started.json", &json!({"checkpoint":"A2","status":"started","order":order.name(),"session_id":session_id,"input_id":input_id}));
    evidence
}

async fn exercise_a2(server: &Server, order: Order) {
    let client = http_client(server);
    let selected = client
        .controller_model_selection()
        .expect("actual registry HTTP selection");
    let endpoint = format!("{}/v1/messages", server.base_url);
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("a2-native-grants"),
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
    let mut operation_ceiling = ceiling("read");
    operation_ceiling.actions = ExactRestriction::exact([action("read"), action("delete")]);
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("record-actions"),
            principal("executor"),
            None,
            operation_ceiling.clone(),
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
    let application = Arc::new(RecordApplication::configured());
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                grants,
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(ResourcePolicy {
                    controller: HttpRecordOwner {
                        selection: selected.clone(),
                        endpoint: endpoint.clone(),
                    },
                    application: application.clone(),
                }),
            })
            .unwrap(),
    );
    let sessions = Arc::new(RecordingStore::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(client);
    builder.default_tool_dispatcher = Some(application.clone());
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
    assert!(pin.selection() == &selected);
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
    let mut prompt = PromptInput::new("Attempt the three record actions", None);
    prompt.header.authority_association = Some(claims.clone());
    let input = Input::Prompt(prompt);
    let input_id = input.id().clone();
    let evidence = evidence_a2(&session_id, &input_id, order);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::super::evidence("a2-current-authentication"),
    )
    .unwrap()
    .with_controller_client(&input, pin)
    .unwrap();
    let input = input.with_ingress_context(current).unwrap();
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .unwrap();
    let completion = completion.expect("actual native completion");
    tokio::time::timeout(
        Duration::from_secs(20),
        server.receiver.second_request.notified(),
    )
    .await
    .expect("all sibling outcomes reach request two in the same run");
    let bodies = server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2);
    for body in &bodies {
        assert_eq!(body["model"], E1_MODEL);
        assert_eq!(body["stream"], true);
    }
    assert_feedback(&bodies[1], order);
    let stored = machine
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .unwrap();
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
    let refused: Vec<_> = audit.iter().filter(|record| matches!(&record.observation.observation,
        AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. } if call_id == DELETE_T && tool_name == "delete_record"))).collect();
    assert_eq!(refused.len(), 1);
    let denied_operation_id = refused[0].observation.operation_id.clone();
    assert!(!audit.iter().any(
        |record| record.observation.operation_id == denied_operation_id
            && matches!(
                record.observation.observation,
                AuditObservation::Entry | AuditObservation::Outcome { .. }
            )
    ));
    let mut tool_ids = vec![denied_operation_id.clone()];
    for (id, name) in [(READ_T, "read_record"), (DELETE_T2, "delete_record")] {
        let prepared: Vec<_> = audit.iter().filter(|record| matches!(&record.observation.observation,
            AuditObservation::Prepared { target, .. }
            if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. } if call_id == id && tool_name == name))).collect();
        assert_eq!(prepared.len(), 1);
        let op = prepared[0].observation.operation_id.clone();
        assert!(!tool_ids.contains(&op));
        assert_eq!(
            audit
                .iter()
                .filter(|record| record.observation.operation_id == op
                    && matches!(record.observation.observation, AuditObservation::Entry))
                .count(),
            1
        );
        assert_eq!(
            audit
                .iter()
                .filter(|record| record.observation.operation_id == op
                    && matches!(
                        &record.observation.observation,
                        AuditObservation::Outcome {
                            outcome: OperationObservedOutcome::ToolDispatchReturned {
                                result_is_error: false,
                                terminal_error: None,
                                ..
                            }
                        }
                    ))
                .count(),
            1
        );
        tool_ids.push(op);
    }
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
    assert_ne!(model_ids[0], model_ids[1]);
    for op in &model_ids {
        assert_eq!(
            audit
                .iter()
                .filter(|record| &record.observation.operation_id == op
                    && matches!(record.observation.observation, AuditObservation::Entry))
                .count(),
            1
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
    application.assert_effects(&run_id);
    let policy_reads = application.policy_reads.lock().unwrap().clone();
    for (id, tool, target) in order.calls() {
        assert!(policy_reads.iter().any(|read| read.call_id == id
            && read.requester == principal("requester")
            && read.target
                == ResourceRef {
                    domain: domain(),
                    resource_id: target.into()
                }
            && read.action == application.actions[tool]));
    }
    evidence.write(
        "requests.json",
        &json!({"order":order.name(),"bodies":bodies}),
    );
    evidence.write("observed.json", &json!({"checkpoint":"A2","status":"observed_before_completion","order":order.name(),"session_id":session_id,"input_id":input_id,"run_id":run_id,"policy_reads":policy_reads,"audit":audit}));
    server.receiver.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("A2 must complete normally in the same run: {outcome:?}")
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
        .unwrap();
    let expected_ids: Vec<_> = order.calls().iter().map(|call| call.0.to_owned()).collect();
    let mut paired = 0;
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
            if ids == expected_ids {
                assert_eq!(results.len(), 3);
                for (id, error, text) in [
                    (DELETE_T, true, None),
                    (READ_T, false, Some(READ_RESULT)),
                    (DELETE_T2, false, Some(DELETE_RESULT)),
                ] {
                    let matching: Vec<_> = results
                        .iter()
                        .filter(|result| result.tool_use_id == id)
                        .collect();
                    assert_eq!(matching.len(), 1);
                    assert_eq!(matching[0].is_error, error);
                    if let Some(text) = text {
                        assert_eq!(matching[0].text_content(), text);
                    }
                }
                paired += 1;
            }
        }
    }
    assert_eq!(
        paired, 1,
        "saved transcript retains the exact sibling ordering and all call results"
    );
    let entries = application.assert_effects(&run_id);
    assert_eq!(server.receiver.bodies.lock().unwrap().len(), 2);
    assert_eq!(
        server.receiver.authorized_requests.load(Ordering::SeqCst),
        2
    );
    evidence.write("completed.json", &json!({
        "checkpoint":"A2","status":"completed","order":order.name(),"session_id":session_id,"input_id":input_id,"run_id":run_id,
        "selected_controller":selected,"endpoint":endpoint,"association":claims,"generated_operation_ceiling":operation_ceiling,
        "tool_operation_ids":tool_ids,"model_operation_ids":model_ids,"body_entries":entries,
        "delete_t_count":entries.iter().filter(|entry| entry.tool == "delete_record" && entry.target.resource_id == TARGET_T).count(),
        "read_t_count":entries.iter().filter(|entry| entry.tool == "read_record" && entry.target.resource_id == TARGET_T).count(),
        "delete_t2_count":entries.iter().filter(|entry| entry.tool == "delete_record" && entry.target.resource_id == TARGET_T2).count(),
        "http_request_count":server.receiver.bodies.lock().unwrap().len(),
        "credential_authorization_count":server.receiver.authorized_requests.load(Ordering::SeqCst),
        "policy_boundary":"configured application resource permission table via OperationPolicyOwner",
        "result":{"text":result.text,"terminal_cause_kind":result.terminal_cause_kind},
        "audit_capture_phase":"second_http_request_waiting_for_response","audit":audit,
    }));
}

async fn run_a2(order: Order) {
    let mut server = start_server(order).await;
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        exercise_a2(&server, order),
    ))
    .catch_unwind()
    .await;
    server.reap().await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("A2 {} timed out: {error}", order.name()),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "A2 requires explicit evidence and the E1 native composition gate"]
async fn adr_a2_resource_specificity_denied_first() {
    run_a2(Order::DeniedFirst).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "A2 requires explicit evidence and the E1 native composition gate"]
async fn adr_a2_resource_specificity_denied_last() {
    run_a2(Order::DeniedLast).await;
}
