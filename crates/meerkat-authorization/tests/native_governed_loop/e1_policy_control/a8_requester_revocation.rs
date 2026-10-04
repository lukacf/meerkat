//! A8: actual native run, application-owned exact permissions and publication.
//! The HTTP model is deterministic. This tests the installed application policy
//! seam, not a shipped universal ABAC store or a live provider.
use super::*;
use serde::Serialize;
use std::collections::BTreeMap;

const FIRST_CALL: &str = "a8-first-append";
const SECOND_CALL: &str = "a8-revoked-delete";
const LAST_CALL: &str = "a8-unrelated-read";
const COMPLETE: &str = "requester revocation preserved this native run";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PermissionRow {
    subject: PrincipalRef,
    action: ActionRef,
    resource: ResourceRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PermissionSnapshot {
    revision: u64,
    rows: Vec<PermissionRow>,
}

fn permission(subject: &str, verb: &str, record: &str) -> PermissionRow {
    PermissionRow {
        subject: principal(subject),
        action: action(verb),
        resource: ResourceRef {
            domain: domain(),
            resource_id: record.into(),
        },
    }
}

/// This table is the configured application's canonical permission state in
/// this fixture. Reads and administrative row removal use this same owner.
/// It is not a mirror of grants and cannot alter executor/controller grants.
struct PermissionTable {
    administrator: PrincipalRef,
    publication: LocalAuthorizationPublication,
    state: Mutex<PermissionSnapshot>,
}

impl PermissionTable {
    fn seeded(publication: LocalAuthorizationPublication) -> Self {
        let rows = ["requester", "executor"]
            .into_iter()
            .flat_map(|subject| {
                [
                    ("append", "record-7"),
                    ("delete", "record-7"),
                    ("read", "record-8"),
                ]
                .into_iter()
                .map(move |(verb, record)| permission(subject, verb, record))
            })
            .collect();
        Self {
            administrator: principal("application-policy-owner"),
            publication,
            state: Mutex::new(PermissionSnapshot { revision: 1, rows }),
        }
    }

    fn snapshot(&self) -> PermissionSnapshot {
        self.state
            .lock()
            .expect("canonical permission table")
            .clone()
    }

    fn remove_permission(
        &self,
        caller: &PrincipalRef,
        row: &PermissionRow,
    ) -> Result<PermissionSnapshot, &'static str> {
        if caller != &self.administrator {
            return Err("only the configured application administrator may mutate permissions");
        }
        // Publication first, owner mutex second. The writer remains held until
        // changed canonical facts are visible. No await or I/O under either.
        let _publication = self
            .publication
            .begin_owner_change()
            .map_err(|_| "publication unavailable")?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "permission owner unavailable")?;
        let index = state
            .rows
            .iter()
            .position(|current| current == row)
            .ok_or("exact permission row absent")?;
        let next_revision = state
            .revision
            .checked_add(1)
            .ok_or("permission revision exhausted")?;
        state.rows.remove(index);
        state.revision = next_revision;
        Ok(state.clone())
    }
}

#[derive(Serialize)]
struct PermissionDecision {
    operation_id: meerkat_core::OperationId,
    call_id: String,
    permission_revision: u64,
    resource: ResourceRef,
    action: ActionRef,
    requester_permitted: bool,
    executor_permitted: bool,
}

struct ApplicationOwner {
    controller: HttpRecordOwner,
    permissions: Arc<PermissionTable>,
    decisions: Mutex<Vec<PermissionDecision>>,
}

impl OperationPolicyOwner for ApplicationOwner {
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
        let arguments: RecordArguments =
            serde_json::from_str(facts.arguments.get()).map_err(|_| denied())?;
        let verb = match facts.name.as_str() {
            "append_record" => "append",
            "delete_record" => "delete",
            "read_record" => "read",
            _ => return Err(denied().into()),
        };
        let actual_resource = ResourceRef {
            domain: domain(),
            resource_id: arguments.record,
        };
        let candidate = association.candidate();
        // Evaluate the correlated exact tuple directly. Grant restrictions stay
        // Cartesian and independently constrain the executor's domain/actions.
        let state = self
            .permissions
            .state
            .lock()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        let allows = |subject: &PrincipalRef| {
            state.rows.iter().any(|row| {
                &row.subject == subject
                    && row.action == action(verb)
                    && row.resource == actual_resource
            })
        };
        let requester_permitted = allows(&candidate.requester);
        let executor_permitted = allows(&candidate.logical_executor);
        self.decisions.lock().unwrap().push(PermissionDecision {
            operation_id: binding.facts().operation_id.clone(),
            call_id: facts.call_id.to_string(),
            permission_revision: state.revision,
            resource: actual_resource,
            action: action(verb),
            requester_permitted,
            executor_permitted,
        });
        if !requester_permitted || !executor_permitted {
            return Err(denied().into());
        }
        Ok(LocalPolicyAllowance {
            operation_values: vec![LocalOperationValues {
                action: action(verb),
                resource_domain: domain(),
                processor: ProcessorRef::Principal {
                    principal: candidate.logical_executor.clone(),
                },
                audience: AudienceRef::Principal {
                    principal: candidate.requester.clone(),
                },
            }],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }
}

#[derive(Clone, Serialize)]
struct PhysicalEntry {
    call_id: String,
    name: String,
    record: String,
    run_id: String,
}

struct PhysicalRecords {
    values: Mutex<BTreeMap<String, i64>>,
    entries: Mutex<Vec<PhysicalEntry>>,
}

impl PhysicalRecords {
    fn seeded() -> Self {
        Self {
            values: Mutex::new(BTreeMap::from([
                ("record-7".into(), 0),
                ("record-8".into(), 80),
            ])),
            entries: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl AgentToolDispatcher for PhysicalRecords {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        ["append_record", "delete_record", "read_record"]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    "isolated application record operation",
                    json!({
                        "type":"object", "properties":{"record":{"type":"string"}},
                        "required":["record"], "additionalProperties":false,
                    }),
                ))
            })
            .collect()
    }

    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("the physical owner requires the actual native context")
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        assert!(context.work_authorization().is_some());
        let run_id = context
            .run_id()
            .expect("native run at physical receiver")
            .to_string();
        let arguments: RecordArguments = serde_json::from_str(call.args.get()).unwrap();
        self.entries.lock().unwrap().push(PhysicalEntry {
            call_id: call.id.into(),
            name: call.name.into(),
            record: arguments.record.clone(),
            run_id,
        });
        let mut values = self.values.lock().unwrap();
        let text = match call.name {
            "append_record" => {
                let value = values
                    .get_mut(&arguments.record)
                    .expect("existing exact record");
                *value += 1;
                format!("{} value={value}", arguments.record)
            }
            "delete_record" => {
                values
                    .remove(&arguments.record)
                    .expect("existing exact record");
                format!("{} deleted", arguments.record)
            }
            "read_record" => format!(
                "{} value={}",
                arguments.record,
                values.get(&arguments.record).unwrap()
            ),
            _ => panic!("unadvertised physical operation"),
        };
        Ok(ToolResult::new(call.id.into(), text, false).into())
    }
}

#[derive(Default)]
struct EntryBarrier {
    second_prepared: Notify,
    release: Notify,
    runs: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl meerkat_core::ToolDispatchAdmission for EntryBarrier {
    async fn await_dispatch_admission(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        _: meerkat_core::LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        let context = context.expect("actual configured admission context");
        assert!(context.work_authorization().is_some());
        self.runs
            .lock()
            .unwrap()
            .push((call.id.into(), context.run_id().unwrap().to_string()));
        if call.id == SECOND_CALL {
            assert_eq!(call.name, "delete_record");
            self.second_prepared.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
}

fn one_tool_response(call_id: &str, name: &str, record: &str) -> String {
    sse(vec![
        start_message(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":call_id,"name":name,"input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":json!({"record":record}).to_string()}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","usage":{"output_tokens":2},"delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ])
}

async fn receive_a8(
    State(receiver): State<Arc<Receiver>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> impl IntoResponse {
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
    let count = {
        let mut bodies = receiver.bodies.lock().unwrap();
        bodies.push(body);
        bodies.len()
    };
    let response = match count {
        1 => one_tool_response(FIRST_CALL, "append_record", "record-7"),
        2 => one_tool_response(SECOND_CALL, "delete_record", "record-7"),
        3 => one_tool_response(LAST_CALL, "read_record", "record-8"),
        4 => {
            receiver.second_request.notify_one();
            receiver.finish.notified().await;
            sse(vec![
                start_message(),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":COMPLETE}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","usage":{"output_tokens":3},"delta":{"stop_reason":"end_turn"}}),
                json!({"type":"message_stop"}),
            ])
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                [("content-type", "application/json")],
                "unexpected extra request".into(),
            );
        }
    };
    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        response,
    )
}

async fn server() -> Server {
    let receiver = Arc::new(Receiver::default());
    let app = Router::new()
        .route("/v1/messages", post(receive_a8))
        .with_state(receiver.clone());
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

fn result_in(body: &Value, call_id: &str) -> Value {
    let results: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_array())
        .flat_map(|content| content.iter())
        .filter(|block| block["type"] == "tool_result" && block["tool_use_id"] == call_id)
        .cloned()
        .collect();
    assert_eq!(
        results.len(),
        1,
        "one real result for exact provider call {call_id}"
    );
    results[0].clone()
}

fn audit_for(stored: &impl Serialize) -> Vec<StoredAuthorizationAuditObservation> {
    serde_json::from_value(serde_json::to_value(stored).unwrap()["authorization_audit"].clone())
        .unwrap()
}

struct A8Evidence {
    directory: PathBuf,
}
impl A8Evidence {
    fn start(session_id: &SessionId, input_id: &meerkat_core::InputId) -> Self {
        let root = ordinary_evidence_root("MEERKAT_A8_EVIDENCE_DIR");
        let directory = root.join(format!("adr-a8-{session_id}"));
        std::fs::create_dir_all(directory.parent().unwrap()).unwrap();
        std::fs::create_dir(&directory).expect("fresh A8 invocation directory");
        let this = Self { directory };
        this.write("started.json", &json!({"checkpoint":"A8","status":"started","session_id":session_id,"input_id":input_id}));
        this
    }
    fn write(&self, name: &str, value: &Value) {
        use std::io::Write;
        let bytes = serde_json::to_vec_pretty(value).unwrap();
        assert!(bytes.len() <= MAX_RECEIPT_BYTES);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.directory.join(name))
            .unwrap();
        file.write_all(&bytes).unwrap();
        file.write_all(b"\n").unwrap();
        file.sync_all().unwrap();
    }
}

async fn exercise_a8(server: &Server, barrier: &Arc<EntryBarrier>) {
    let client = http_client(server);
    let selected = client.controller_model_selection().unwrap();
    let endpoint = format!("{}/v1/messages", server.base_url);
    let publication = LocalAuthorizationPublication::new();
    let permissions = Arc::new(PermissionTable::seeded(publication.clone()));
    let policy = Arc::new(ApplicationOwner {
        controller: HttpRecordOwner {
            selection: selected.clone(),
            endpoint: endpoint.clone(),
        },
        permissions: permissions.clone(),
        decisions: Mutex::new(Vec::new()),
    });
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("a8-native-grants"),
                generation: 1,
            },
            publication,
            Arc::new(HostAuthorizationClock),
        )
        .unwrap(),
    );
    let mut operation_bounds = ceiling("read");
    operation_bounds.actions =
        ExactRestriction::exact([action("append"), action("delete"), action("read")]);
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("a8-controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .unwrap();
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("a8-operations"),
            principal("executor"),
            None,
            operation_bounds.clone(),
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
                grants: grants.clone(),
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: policy.clone(),
            })
            .unwrap(),
    );
    let physical = Arc::new(PhysicalRecords::seeded());
    let sessions = Arc::new(RecordingStore::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(client);
    builder.default_tool_dispatcher = Some(Arc::new(
        meerkat_core::tool_execution_policy::ExecutionPolicyGatedDispatcher::new(
            physical.clone(),
            meerkat_core::ToolExecutionPolicy::unrestricted(),
        )
        .with_dispatch_admission(barrier.clone()),
    ));
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
    let claims = association(
        &LogicalRuntimeId::for_session(&session_id),
        controller.clone(),
        operation.clone(),
        selected.clone(),
    );
    let mut prompt = PromptInput::new(
        "Append once, attempt delete, then read the unrelated record",
        None,
    );
    prompt.header.authority_association = Some(claims.clone());
    let input = Input::Prompt(prompt);
    let input_id = input.id().clone();
    let evidence = A8Evidence::start(&session_id, &input_id);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::super::evidence("a8-current-authentication"),
    )
    .unwrap()
    .with_controller_client(&input, pin)
    .unwrap();
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input.with_ingress_context(current).unwrap())
        .await
        .unwrap();
    let completion = completion.expect("native completion receipt");
    tokio::time::timeout(Duration::from_secs(20), barrier.second_prepared.notified())
        .await
        .expect("second call held after native preparation");
    assert_eq!(server.receiver.bodies.lock().unwrap().len(), 2);
    let first_entries = physical.entries.lock().unwrap().clone();
    assert_eq!(first_entries.len(), 1);
    assert_eq!(first_entries[0].call_id, FIRST_CALL);
    assert_eq!(physical.values.lock().unwrap()["record-7"], 1);
    let before = permissions.snapshot();
    let revoke_row = permission("requester", "delete", "record-7");
    assert!(before.rows.contains(&revoke_row));
    assert!(
        policy
            .decisions
            .lock()
            .unwrap()
            .iter()
            .any(|decision| decision.call_id == SECOND_CALL
                && decision.permission_revision == before.revision
                && decision.requester_permitted
                && decision.executor_permitted)
    );
    let operation_before = grants
        .resolve_lineage(
            std::slice::from_ref(&operation),
            &principal("executor"),
            None,
        )
        .unwrap()
        .restrictions()
        .clone();
    let controller_before = grants
        .resolve_controller_lineage(
            std::slice::from_ref(&controller),
            &principal("executor"),
            None,
        )
        .unwrap()
        .restrictions()
        .clone();
    let held = machine
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    let held_audit = audit_for(&held);
    let run_id = held_audit
        .first()
        .unwrap()
        .observation
        .run_id
        .clone()
        .unwrap();
    let first_operation = held_audit
        .iter()
        .find(|record| {
            matches!(&record.observation.observation,
        AuditObservation::Prepared { target, .. }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
            if call_id == FIRST_CALL && tool_name == "append_record"))
        })
        .unwrap()
        .observation
        .operation_id
        .clone();
    let second_operation = held_audit
        .iter()
        .find(|record| {
            matches!(&record.observation.observation,
        AuditObservation::Prepared { target, .. }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
            if call_id == SECOND_CALL && tool_name == "delete_record"))
        })
        .unwrap()
        .observation
        .operation_id
        .clone();
    assert!(first_operation != second_operation);
    assert!(
        held_audit
            .iter()
            .any(|record| record.observation.operation_id == first_operation
                && matches!(
                    &record.observation.observation,
                    AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::ToolDispatchReturned {
                            result_is_error: false,
                            terminal_error: None,
                            ..
                        }
                    }
                )),
        "first effect returned successfully before the permission mutation"
    );
    evidence.write("held-before-revocation.json", &json!({
        "session_id":session_id,"input_id":input_id,"run_id":run_id,"permissions":before,
        "physical_entries":first_entries,"physical_records":*physical.values.lock().unwrap(),"audit":held_audit,
    }));
    let after = permissions
        .remove_permission(&principal("application-policy-owner"), &revoke_row)
        .unwrap();
    let mut expected = before.clone();
    expected.rows.retain(|row| row != &revoke_row);
    expected.revision += 1;
    assert_eq!(
        after, expected,
        "exactly the requester delete permission changed"
    );
    assert!(
        after
            .rows
            .contains(&permission("executor", "delete", "record-7"))
    );
    assert!(
        after
            .rows
            .contains(&permission("requester", "read", "record-8"))
    );
    assert_eq!(
        *grants
            .resolve_lineage(
                std::slice::from_ref(&operation),
                &principal("executor"),
                None
            )
            .unwrap()
            .restrictions(),
        operation_before
    );
    assert_eq!(operation_before, operation_bounds);
    assert_eq!(
        *grants
            .resolve_controller_lineage(
                std::slice::from_ref(&controller),
                &principal("executor"),
                None
            )
            .unwrap()
            .restrictions(),
        controller_before
    );
    evidence.write("requester-permission-removed.json", &json!({"before":before,"after":after,"removed":revoke_row,"controller_grant":controller,"operation_grant":operation}));
    barrier.release.notify_one();
    tokio::time::timeout(
        Duration::from_secs(20),
        server.receiver.second_request.notified(),
    )
    .await
    .expect("unrelated result reaches fourth controller request");
    let bodies = server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 4);
    for body in &bodies {
        assert_eq!(body["model"], E1_MODEL);
        assert_eq!(body["stream"], true);
    }
    assert_eq!(
        wire_text(&result_in(&bodies[1], FIRST_CALL)["content"]),
        "record-7 value=1"
    );
    let refused = result_in(&bodies[2], SECOND_CALL);
    assert_eq!(refused["is_error"], true);
    let refused_payload: Value = serde_json::from_str(&wire_text(&refused["content"])).unwrap();
    assert_eq!(
        refused_payload,
        ToolError::AuthorizationRefused { refusal: denied() }.to_error_payload()
    );
    assert_eq!(
        wire_text(&result_in(&bodies[3], LAST_CALL)["content"]),
        "record-8 value=80"
    );
    assert_ne!(result_in(&bodies[3], LAST_CALL)["is_error"], true);
    let entries = physical.entries.lock().unwrap().clone();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.call_id.as_str())
            .collect::<Vec<_>>(),
        [FIRST_CALL, LAST_CALL]
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry.run_id == run_id.to_string())
    );
    {
        let runs = barrier.runs.lock().unwrap();
        assert_eq!(
            runs.iter()
                .map(|(call, _)| call.as_str())
                .collect::<Vec<_>>(),
            [FIRST_CALL, SECOND_CALL, LAST_CALL]
        );
        assert!(runs.iter().all(|(_, run)| run == &run_id.to_string()));
    }
    assert_eq!(
        *physical.values.lock().unwrap(),
        BTreeMap::from([("record-7".into(), 1), ("record-8".into(), 80)])
    );
    {
        let decisions = policy.decisions.lock().unwrap();
        assert!(
            decisions
                .iter()
                .any(|decision| decision.call_id == SECOND_CALL
                    && decision.operation_id == second_operation
                    && decision.permission_revision == after.revision
                    && !decision.requester_permitted
                    && decision.executor_permitted)
        );
        assert!(
            decisions
                .iter()
                .any(|decision| decision.call_id == LAST_CALL
                    && decision.permission_revision == after.revision
                    && decision.requester_permitted
                    && decision.executor_permitted)
        );
        evidence.write(
            "owner-decisions.json",
            &serde_json::to_value(&*decisions).unwrap(),
        );
    }
    let stored = machine
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    let audit = audit_for(&stored);
    for record in &audit {
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(record.contributors[0].input_id, input_id);
        assert!(record.contributors[0].requester == principal("requester"));
        assert!(record.contributors[0].logical_executor == principal("executor"));
        assert_eq!(record.observation.run_id.as_ref(), Some(&run_id));
        assert!(
            matches!(&record.observation.execution_scope, OperationExecutionScope::RuntimeInput {
            owner_session_id, submitted_input_id, canonical_input_id, ..
        } if owner_session_id == &session_id && submitted_input_id == &input_id && canonical_input_id == &input_id)
        );
    }
    let denied_operation = audit
        .iter()
        .find(|record| {
            matches!(&record.observation.observation,
        AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
            if call_id == SECOND_CALL && tool_name == "delete_record"))
        })
        .unwrap();
    assert_eq!(denied_operation.observation.operation_id, second_operation);
    assert!(
        audit
            .iter()
            .any(|record| record.observation.operation_id == second_operation
                && matches!(&record.observation.observation, AuditObservation::Outcome {
            outcome: OperationObservedOutcome::ToolDispatchError {
                error: meerkat_core::ops::ToolDispatchTerminalErrorKind::AuthorizationRefused
            }
        })),
        "root dispatch preserves the exact operation's typed refusal outcome"
    );
    assert!(
        audit.starts_with(&held_audit),
        "the ordered first-effect audit prefix remains unchanged"
    );
    // Generic fenced dispatch already staged an Entry before the configured
    // admission wait. Only the downstream physical receiver must stay at zero.
    // No successful physical return may be recorded for the revoked operation.
    assert!(!audit.iter().any(|record| record.observation.operation_id
        == denied_operation.observation.operation_id
        && matches!(
            &record.observation.observation,
            AuditObservation::Outcome {
                outcome: OperationObservedOutcome::ToolDispatchReturned {
                    result_is_error: false,
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
    assert_eq!(model_ids.len(), 4);
    evidence.write("observed-before-completion.json", &json!({
        "checkpoint":"A8","session_id":session_id,"input_id":input_id,"run_id":run_id,
        "association":claims,"selected_controller":selected,"endpoint":endpoint,"model_requests":bodies,
        "permissions":after,"physical_entries":entries,"audit":audit,
    }));
    server.receiver.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("same native run must complete: {outcome:?}")
    };
    assert_eq!(result.text, COMPLETE);
    assert_eq!(result.session_id, session_id);
    assert!(result.terminal_cause_kind.is_none());
    let saved = sessions
        .0
        .lock()
        .unwrap()
        .get(&session_id)
        .cloned()
        .unwrap();
    for (call_id, is_error, text) in [
        (FIRST_CALL, false, "record-7 value=1"),
        (SECOND_CALL, true, ""),
        (LAST_CALL, false, "record-8 value=80"),
    ] {
        let retained: Vec<&ToolResult> = saved
            .messages()
            .iter()
            .flat_map(|message| match message {
                Message::ToolResults { results, .. } => results.as_slice(),
                _ => &[],
            })
            .filter(|result| result.tool_use_id == call_id)
            .collect();
        assert_eq!(
            retained.len(),
            1,
            "one canonical saved result for exact call {call_id}"
        );
        assert_eq!(retained[0].is_error, is_error);
        if is_error {
            assert_eq!(
                serde_json::from_str::<Value>(&retained[0].text_content()).unwrap(),
                refused_payload
            );
        } else {
            assert_eq!(retained[0].text_content(), text);
        }
    }
    let final_pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .unwrap();
    assert!(final_pin.selection() == &selected);
    let final_entries = physical.entries.lock().unwrap().clone();
    let final_records = physical.values.lock().unwrap().clone();
    let first_mutations = final_entries
        .iter()
        .filter(|entry| entry.call_id == FIRST_CALL)
        .count();
    let second_receiver_entries = final_entries
        .iter()
        .filter(|entry| entry.call_id == SECOND_CALL)
        .count();
    let unrelated_reads = final_entries
        .iter()
        .filter(|entry| entry.call_id == LAST_CALL)
        .count();
    let model_http_requests = server.receiver.bodies.lock().unwrap().len();
    let authorized_requests = server.receiver.authorized_requests.load(Ordering::SeqCst);
    assert_eq!(
        (first_mutations, second_receiver_entries, unrelated_reads),
        (1, 0, 1)
    );
    assert_eq!(
        final_entries
            .iter()
            .map(|entry| entry.call_id.as_str())
            .collect::<Vec<_>>(),
        [FIRST_CALL, LAST_CALL]
    );
    assert!(
        final_entries
            .iter()
            .all(|entry| entry.run_id == run_id.to_string())
    );
    assert_eq!(
        final_records,
        BTreeMap::from([("record-7".into(), 1), ("record-8".into(), 80)])
    );
    assert_eq!(model_http_requests, 4);
    assert_eq!(authorized_requests, model_http_requests);
    evidence.write("completed.json", &json!({
        "checkpoint":"A8","status":"completed","session_id":session_id,"input_id":input_id,"run_id":run_id,
        "controller_grant":controller,"operation_grant":operation,"selected_controller":selected,
        "requester_permission_removed":revoke_row,"first_operation_id":first_operation,"second_operation_id":second_operation,"first_mutations":first_mutations,"second_receiver_entries":second_receiver_entries,"unrelated_reads":unrelated_reads,
        "physical_entries":final_entries,"physical_records":final_records,
        "model_http_requests":model_http_requests,"authorized_requests":authorized_requests,"result":{"text":result.text,"terminal_cause_kind":result.terminal_cause_kind},
        "scope":"deterministic HTTP and actual application table/native owners; not live-provider or generic ABAC coverage",
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adr_a8_requester_permission_revocation_preserves_effect_and_controller() {
    let mut server = server().await;
    let barrier = Arc::new(EntryBarrier::default());
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        exercise_a8(&server, &barrier),
    ))
    .catch_unwind()
    .await;
    barrier.release.notify_one();
    server.receiver.finish.notify_one();
    server.reap().await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("A8 timed out: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
