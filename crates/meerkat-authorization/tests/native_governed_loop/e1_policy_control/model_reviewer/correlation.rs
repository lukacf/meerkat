//! Concurrent native review audit joins, including a real SQLite owner reopen.
//! HTTP arrival order controls the experiment only. Every historical join is
//! reconstructed from the protected candidate/start/child records themselves.

use super::*;
use meerkat::surface::{
    PersistentRuntimeExecutor, SurfaceSessionRecoveryContext, SurfaceSessionRecoveryOverrides,
    build_recovered_session, build_runtime_backed_service_with_default_reconfigure_host,
    materialize_session_with_reserved_admission_and_actor_slot,
};
use meerkat_authorization_contracts::audit::{AuditModelUse, AuditReviewRole, AuditSourceUse};
use meerkat_core::{InputId, OperationId};
use meerkat_runtime::RuntimeStore;
use meerkat_runtime::input_state::StoredInputState;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Weak;

const CALL_A: &str = "concurrent-review-a";
const CALL_B: &str = "concurrent-review-b";
const SIBLING: &str = "concurrent-review-r1";
const FRESH_GOAL: &str =
    "After owner reopen, repeat these operations with fresh current admission.";
type Service = meerkat::PersistentSessionService<FactoryAgentBuilder>;
type Cleanup = Mutex<Option<(Arc<MeerkatMachine>, SessionId)>>;

fn concurrent_response() -> String {
    let mut events = vec![start_message()];
    for (index, call, tool) in [
        (0, CALL_A, "delete_record"),
        (1, CALL_B, "delete_record"),
        (2, SIBLING, "read_record"),
    ] {
        events.extend([
            json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":call,"name":tool,"input":{}}}),
            json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":"{\"record\":\"record-7\"}"}}),
            json!({"type":"content_block_stop","index":index}),
        ]);
    }
    events.extend([
        json!({"type":"message_delta","usage":{"output_tokens":3},"delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ]);
    sse(events)
}

#[derive(Default)]
struct CorrelationTools {
    inner: RecordingTools,
    entries: Mutex<Vec<String>>,
}

#[async_trait]
impl AgentToolDispatcher for CorrelationTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.inner.tools()
    }
    fn review_entry_support(&self, _: &str) -> ReviewEntrySupport {
        ReviewEntrySupport::ConsumesAtEntry
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("the actual reviewed leaf must retain its native context")
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let _entry = context.enter_reviewed_effect(call, None)?;
        self.entries.lock().unwrap().push(call.id.to_owned());
        self.inner.dispatch(call).await
    }
}

fn stored_audit(row: &StoredInputState) -> Vec<StoredAuthorizationAuditObservation> {
    #[derive(Deserialize)]
    struct ProtectedAudit {
        #[serde(default)]
        authorization_audit: Vec<StoredAuthorizationAuditObservation>,
    }
    serde_json::from_value::<ProtectedAudit>(serde_json::to_value(row).unwrap())
        .expect("actual protected native row audit")
        .authorization_audit
}

fn target(record: &StoredAuthorizationAuditObservation) -> Option<&AuditTarget> {
    match &record.observation.observation {
        AuditObservation::Prepared { target, .. }
        | AuditObservation::Refused { target, .. }
        | AuditObservation::AuthorizationUnavailable { target } => Some(target.as_ref()),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct HistoricalReview {
    candidate: OperationId,
    attempt: String,
    source: OperationId,
    inference: OperationId,
}

/// No caller-provided attempt map, live review observer, or request order is
/// an input to this reconstruction. Equal model/original facts cannot stand
/// in for the exact owner-issued candidate/attempt relationship.
fn protected_graph(
    audit: &[StoredAuthorizationAuditObservation],
    session: &SessionId,
    input: &InputId,
    reviewer: &ControllerModelSelection,
    endpoint: &str,
    completed_models: &[&str],
) -> BTreeMap<String, HistoricalReview> {
    let starts: Vec<_> = audit
        .iter()
        .enumerate()
        .filter_map(|(index, record)| {
            if let AuditObservation::ReviewAttemptStarted { attempt_ref } =
                &record.observation.observation
            {
                Some((index, record, attempt_ref))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(starts.len(), 2, "two actual R2 attempts, no R1 attempt");
    assert!(starts[0].1.observation.run_id.is_some());
    assert_eq!(
        starts[0].1.observation.run_id,
        starts[1].1.observation.run_id
    );
    assert_eq!(
        starts[0].1.observation.execution_scope,
        starts[1].1.observation.execution_scope
    );
    assert_ne!(
        starts[0].1.observation.operation_id,
        starts[1].1.observation.operation_id
    );
    assert_ne!(
        starts[0].2, starts[1].2,
        "same-run attempts are not interchangeable"
    );
    let mut graph = BTreeMap::new();
    let mut child_ids = BTreeSet::new();
    let mut argument_digests = Vec::new();
    for (start_index, start, attempt) in &starts {
        assert!(!attempt.is_empty());
        assert!(start.observation.review_attribution.is_none());
        let candidate = &start.observation.operation_id;
        let candidate_target = audit
            .iter()
            .filter(|record| &record.observation.operation_id == candidate)
            .find_map(|record| match target(record) {
                Some(AuditTarget::Tool {
                    call_id,
                    tool_name,
                    arguments_digest,
                    ..
                }) => Some((call_id, tool_name, arguments_digest)),
                _ => None,
            })
            .expect("original candidate's protected target supplies the call identity");
        let (call, name, digest) = candidate_target;
        assert!([CALL_A, CALL_B].contains(&call.as_str()));
        assert_eq!(name, "delete_record");
        argument_digests.push(*digest);
        let children: Vec<_> = audit
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                record
                    .observation
                    .review_attribution
                    .as_ref()
                    .is_some_and(|attribution| {
                        &attribution.candidate_operation_id == candidate
                            && &attribution.attempt_ref == *attempt
                    })
            })
            .collect();
        assert!(!children.is_empty());
        let mut operations = Vec::new();
        for (index, record) in &children {
            assert!(
                *index > *start_index,
                "Started precedes every child observation"
            );
            assert_eq!(record.contributors.len(), 1);
            assert_eq!(&record.contributors[0].input_id, input);
            assert!(record.contributors[0].requester == principal("requester"));
            assert!(record.contributors[0].logical_executor == principal("executor"));
            assert_eq!(
                record.observation.context_revision,
                start.observation.context_revision
            );
            assert_eq!(record.observation.run_id, start.observation.run_id);
            assert_eq!(
                record.observation.execution_scope,
                start.observation.execution_scope
            );
            if !operations.contains(&record.observation.operation_id) {
                operations.push(record.observation.operation_id.clone());
            }
        }
        assert_eq!(
            operations.len(),
            2,
            "one exact source read and one reviewer inference per attempt"
        );
        let mut source = None;
        let mut inference = None;
        for operation in operations {
            assert!(
                !starts
                    .iter()
                    .any(|(_, record, _)| record.observation.operation_id == operation)
            );
            assert!(
                child_ids.insert(operation.to_string()),
                "children cannot belong to two candidates"
            );
            let records: Vec<_> = audit
                .iter()
                .filter(|record| record.observation.operation_id == operation)
                .collect();
            let attribution = records[0].observation.review_attribution.as_ref().unwrap();
            for record in &records {
                assert!(
                    record.observation.review_attribution.as_ref() == Some(attribution),
                    "Prepared/refusal/unavailable/Entry/Outcome all retain the same pair and role"
                );
            }
            assert_eq!(
                records
                    .iter()
                    .filter(|record| matches!(
                        record.observation.observation,
                        AuditObservation::Entry
                    ))
                    .count(),
                1
            );
            let prepared: Vec<_> = records
                .iter()
                .filter(|record| {
                    matches!(
                        record.observation.observation,
                        AuditObservation::Prepared { .. }
                    )
                })
                .collect();
            assert_eq!(prepared.len(), 1);
            match attribution.role {
                AuditReviewRole::ContextRead => {
                    assert!(
                        matches!(target(prepared[0]), Some(AuditTarget::RuntimeInput {
                        owner_session_id, input_id, usage: AuditSourceUse::Read, ..
                    }) if owner_session_id == session && input_id == input)
                    );
                    assert_eq!(
                        records
                            .iter()
                            .filter(|record| matches!(
                                record.observation.observation,
                                AuditObservation::Outcome {
                                    outcome: OperationObservedOutcome::SourceReadMaterialized
                                }
                            ))
                            .count(),
                        1
                    );
                    assert!(source.replace(operation).is_none());
                }
                AuditReviewRole::ReviewerInference => {
                    assert!(
                        matches!(target(prepared[0]), Some(AuditTarget::Model(model))
                        if model.usage == AuditModelUse::Inference && model.endpoint == endpoint
                            && model.credential.as_ref() == Some(reviewer.credential()) && model.wire_model == E1_MODEL)
                    );
                    assert_eq!(
                        records
                            .iter()
                            .filter(|record| matches!(
                                record.observation.observation,
                                AuditObservation::Outcome {
                                    outcome: OperationObservedOutcome::HttpResponse { status: 200 }
                                }
                            ))
                            .count(),
                        usize::from(completed_models.contains(&call.as_str())),
                        "reverse completion remains joined to the candidate whose actual response returned"
                    );
                    assert!(inference.replace(operation).is_none());
                }
            }
        }
        assert!(
            graph
                .insert(
                    call.clone(),
                    HistoricalReview {
                        candidate: candidate.clone(),
                        attempt: (*attempt).clone(),
                        source: source.unwrap(),
                        inference: inference.unwrap(),
                    }
                )
                .is_none()
        );
    }
    assert_eq!(
        argument_digests[0], argument_digests[1],
        "equal proposed effects still need distinct attempt joins"
    );
    for record in audit {
        if let Some(attribution) = &record.observation.review_attribution {
            assert!(
                graph.values().any(
                    |review| review.candidate == attribution.candidate_operation_id
                        && review.attempt == attribution.attempt_ref
                ),
                "no orphan or mismatched child attribution"
            );
        }
    }
    graph
}

fn successful_tool_outcome(audit: &[StoredAuthorizationAuditObservation], call: &str) -> bool {
    let candidates: Vec<_> = audit
        .iter()
        .filter_map(|record| match target(record) {
            Some(AuditTarget::Tool { call_id, .. }) if call_id == call => {
                Some(&record.observation.operation_id)
            }
            _ => None,
        })
        .collect();
    audit.iter().any(|record| {
        candidates.contains(&&record.observation.operation_id)
            && matches!(
                record.observation.observation,
                AuditObservation::Outcome {
                    outcome: OperationObservedOutcome::ToolDispatchReturned {
                        result_is_error: false,
                        terminal_error: None,
                        ..
                    }
                }
            )
    })
}

fn persistent_service(
    database: &std::path::Path,
    factory: AgentFactory,
    config: Config,
    client: Arc<dyn LlmClient>,
    tools: Arc<CorrelationTools>,
    authorization: NativeGrantWorkConfiguration,
) -> (
    Arc<Service>,
    Arc<MeerkatMachine>,
    Arc<dyn RuntimeStore>,
    Weak<dyn meerkat::SessionStore>,
) {
    let sessions: Arc<dyn meerkat::SessionStore> =
        Arc::new(meerkat::SqliteSessionStore::open(database.to_path_buf()).unwrap());
    let old_sessions = Arc::downgrade(&sessions);
    let store: Arc<dyn RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_whole_blob(database.to_path_buf()).unwrap(),
    );
    let bundle = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        sessions,
        store.clone(),
        Arc::new(meerkat::MemoryBlobStore::new()),
        authorization,
    )
    .unwrap();
    let mut builder = FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(client);
    builder.default_tool_dispatcher = Some(tools);
    let (service, machine) = build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        2,
        bundle,
        std::env::temp_dir().join(format!("review-correlation-{}.toml", SessionId::new())),
    );
    (service, machine, store, old_sessions)
}

async fn materialize(
    service: &Arc<Service>,
    machine: &Arc<MeerkatMachine>,
    seed: Session,
    request: CreateSessionRequest,
) {
    let reserved = service.reserve_create_session_admission().await.unwrap();
    Box::pin(materialize_session_with_reserved_admission_and_actor_slot(
        service,
        machine,
        seed,
        request,
        reserved,
        {
            let service = service.clone();
            let machine = machine.clone();
            move |session, _attachment, actor_slot| {
                Box::new(
                    PersistentRuntimeExecutor::new(service, machine, session)
                        .with_publication_actor_slot(actor_slot),
                )
            }
        },
    ))
    .await
    .unwrap();
}

fn review_call(body: &Value) -> String {
    let request: Value = serde_json::from_str(&wire_text(&body["messages"][0]["content"])).unwrap();
    request["proposed_operation"]["call_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn respond(reply: &ControlledReviewReply, case: Case) {
    assert!(
        reply
            .response
            .lock()
            .unwrap()
            .replace(review_response(case))
            .is_none()
    );
    reply.release.notify_one();
}

fn assert_feedback(body: &Value, allowed: Option<&str>) {
    let results: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .collect();
    // The second run deliberately reuses call IDs. Only its final batch is
    // tested here; historical results cannot satisfy a fresh refusal oracle.
    let results = &results[results.len().checked_sub(3).unwrap()..];
    for call in [CALL_A, CALL_B, SIBLING] {
        let result = results
            .iter()
            .find(|result| result["tool_use_id"] == call)
            .unwrap();
        if call == SIBLING || Some(call) == allowed {
            assert_ne!(result["is_error"], true);
            assert_eq!(wire_text(&result["content"]), "record-7 value");
        } else {
            let feedback = if allowed.is_some() {
                ToolError::ReviewUnsatisfied {
                    kind: ReviewUnsatisfiedKind::Denied,
                }
            } else {
                ToolError::ReviewUnavailable {
                    kind: ReviewUnavailableKind::ReviewerMissing,
                }
            };
            assert_eq!(result["is_error"], true);
            assert_eq!(
                serde_json::from_str::<Value>(&wire_text(&result["content"])).unwrap(),
                feedback.to_error_payload()
            );
        }
    }
}

async fn exercise(
    controller_server: &Server,
    reviewer_server: &Server,
    database: &std::path::Path,
    cleanup: &Cleanup,
) {
    let client = http_client(controller_server);
    let selected = client.controller_model_selection().unwrap();
    let mut config = Config::default();
    config.tools.max_concurrent = 3;
    let mut realm =
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[("anthropic", REVIEW_KEY)]);
    realm.backend.get_mut("default_anthropic").unwrap().base_url =
        Some(reviewer_server.base_url.clone());
    config.realm.insert("native_review".into(), realm);
    let reviewer_identity = SessionLlmIdentity {
        model: E1_MODEL.into(),
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(AuthBindingRef {
            realm: RealmId::parse("native_review").unwrap(),
            binding: BindingId::parse("default_anthropic").unwrap(),
            profile: None,
            origin: BindingOrigin::Configured,
        }),
    };
    let factory = AgentFactory::minimal().without_provider_auth_persistence();
    let reviewer_selection = factory
        .build_llm_client_for_identity(&config, &reviewer_identity)
        .await
        .unwrap()
        .controller_model_selection()
        .unwrap();
    let reviewer = ModelOperationReviewer::build(
        &factory,
        &config,
        ModelReviewerConfig::new(reviewer_identity, 73).unwrap(),
        Arc::new(NativeOperationReviewContextSource),
    )
    .await
    .unwrap();
    // The bound owner is moved into the original service. Its Weak below must
    // expire before reopen; persisted audit never reconstructs this authority.
    let review = Arc::new(BoundOperationReview::new(
        Arc::new(reviewer),
        meerkat_core::ApprovalService::new(),
        Duration::from_secs(20),
    ));
    let old_review = Arc::downgrade(&review);
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("review-correlation-grants"),
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
    let mut bounds = ceiling("read");
    bounds.actions = ExactRestriction::exact([action("read"), action("delete"), action("infer")]);
    bounds.resource_domains = ExactRestriction::exact([domain(), input_domain()]);
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("reviewable-operations"),
            principal("executor"),
            None,
            bounds,
        )
        .unwrap();
    let ingress_controller = controller.clone();
    let ingress_operation = operation.clone();
    let ingress_selection = selected.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("native-loop").unwrap()
            || claimed
                != &association(
                    runtime,
                    ingress_controller.clone(),
                    ingress_operation.clone(),
                    ingress_selection.clone(),
                )
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let authorization = |original: &InputId| NativeGrantWorkConfiguration {
        grants: grants.clone(),
        ingress: ingress.clone(),
        invocation_owner: Arc::new(InvocationOwner),
        operation_owner: Arc::new(ReviewPolicy {
            controller: HttpRecordOwner {
                selection: selected.clone(),
                endpoint: format!("{}/v1/messages", controller_server.base_url),
            },
            reviewer: reviewer_selection.clone(),
            reviewer_endpoint: format!("{}/v1/messages", reviewer_server.base_url),
            original: original.clone(),
            source_case: Case::Allow,
            source_checks: Arc::new(AtomicUsize::new(0)),
            model_probe: None,
        }),
    };
    let tokens = meerkat_core::auth::ProviderAuthPersistence::new(
        Arc::new(meerkat_auth_core::EphemeralTokenStore::new()),
        Arc::new(meerkat_auth_core::InMemoryCoordinator::new()),
    );
    let tools = Arc::new(CorrelationTools::default());
    let mut prompt = PromptInput::new(GOAL, None);
    let input_id = prompt.header.id.clone();
    let (service, machine, store, old_sessions) = persistent_service(
        database,
        factory.with_operation_review(review),
        config.clone(),
        client.clone(),
        tools.clone(),
        authorization(&input_id),
    );
    let old_machine = Arc::downgrade(&machine);
    let old_store = Arc::downgrade(&store);
    let seed = Session::new();
    let session_id = seed.id().clone();
    *cleanup.lock().unwrap() = Some((machine.clone(), session_id.clone()));
    materialize(
        &service,
        &machine,
        seed,
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
    )
    .await;
    let pin = {
        let lease = service
            .acquire_live_session_actor_turn_boundary_lease(&session_id)
            .await
            .unwrap();
        service
            .pin_controller_client_for_actor(&lease)
            .await
            .unwrap()
    };
    meerkat_auth_core::save_tokens_and_publish_lifecycle(
        tokens.clone(),
        machine.generated_auth_lease_handle(),
        selected.credential().clone(),
        meerkat_core::auth::PersistedTokens::api_key("synthetic-e1-loopback-only"),
    )
    .await
    .unwrap();
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let claims = association(
        &runtime,
        controller.clone(),
        operation.clone(),
        selected.clone(),
    );
    prompt.header.authority_association = Some(claims.clone());
    let input = Input::Prompt(prompt);
    let expected_input = serde_json::to_value(&input).unwrap();
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::super::super::evidence("concurrent-native-review-authentication"),
    )
    .unwrap()
    .with_controller_client(&input, pin)
    .unwrap();
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input.with_ingress_context(current).unwrap())
        .await
        .unwrap();
    let completion = completion.unwrap();
    for call in [CALL_A, CALL_B] {
        tokio::time::timeout(
            Duration::from_secs(10),
            reviewer_server.receiver.controlled_review_replies[call]
                .arrived
                .notified(),
        )
        .await
        .unwrap();
    }
    let bodies = reviewer_server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(
        bodies.len(),
        2,
        "both real reviewer requests are held concurrently"
    );
    let denied_call = review_call(&bodies[0]);
    let allowed_call = review_call(&bodies[1]);
    assert_ne!(denied_call, allowed_call);
    for body in &bodies {
        let request: Value =
            serde_json::from_str(&wire_text(&body["messages"][0]["content"])).unwrap();
        let context: Value =
            serde_json::from_str(request["owner_supplied_context"].as_str().unwrap()).unwrap();
        assert_eq!(
            context["original_inputs"],
            json!([{
                "input_id":input_id, "input":expected_input, "authenticated_association":claims,
            }]),
            "both attempts read the exact same admitted original, not a transcript identity"
        );
    }
    respond(
        &reviewer_server.receiver.controlled_review_replies[allowed_call.as_str()],
        Case::Allow,
    );
    // Observe actual owner outcome after B enters while A is still held. A
    // swapped child attribution cannot hide behind two eventual HTTP 200s.
    let partial = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let row = machine
                .input_state(&session_id, &input_id)
                .await
                .unwrap()
                .unwrap();
            let audit = stored_audit(&row);
            if successful_tool_outcome(&audit, &allowed_call) {
                break audit;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(tools.entries.lock().unwrap().contains(&allowed_call));
    assert!(!tools.entries.lock().unwrap().contains(&denied_call));
    let endpoint = format!("{}/v1/messages", reviewer_server.base_url);
    let partial_graph = protected_graph(
        &partial,
        &session_id,
        &input_id,
        &reviewer_selection,
        &endpoint,
        &[&allowed_call],
    );
    respond(
        &reviewer_server.receiver.controlled_review_replies[denied_call.as_str()],
        Case::Deny,
    );
    tokio::time::timeout(
        Duration::from_secs(10),
        controller_server.receiver.second_request.notified(),
    )
    .await
    .unwrap();
    let controller_bodies = controller_server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(controller_bodies.len(), 2);
    assert_feedback(&controller_bodies[1], Some(&allowed_call));
    controller_server.receiver.finish.notify_one();
    let outcome = completion.wait().await.unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("review denial is local: {outcome:?}")
    };
    assert_eq!(result.text, FINISHED);
    assert_eq!(result.session_id, session_id);
    assert!(result.terminal_cause_kind.is_none());
    let final_row = store
        .load_input_state(&runtime, &input_id)
        .await
        .unwrap()
        .unwrap();
    let audit = stored_audit(&final_row);
    let graph = protected_graph(
        &audit,
        &session_id,
        &input_id,
        &reviewer_selection,
        &endpoint,
        &[CALL_A, CALL_B],
    );
    assert_eq!(
        graph, partial_graph,
        "completion adds outcomes, not different identity joins"
    );
    assert!(successful_tool_outcome(&audit, &allowed_call));
    assert!(!successful_tool_outcome(&audit, &denied_call));
    assert!(successful_tool_outcome(&audit, SIBLING));
    assert_eq!(
        tools
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.as_str() == allowed_call)
            .count(),
        1
    );
    assert_eq!(
        tools
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.as_str() == SIBLING)
            .count(),
        1
    );
    let frozen_row = serde_json::to_value(&final_row).unwrap();
    let document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    let frozen_document = document.bytes().to_vec();
    assert!(
        document
            .session()
            .messages()
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.text_content() == GOAL))
    );
    drop(document);
    machine
        .unregister_current_session_registration_until_terminal(&session_id)
        .await
        .unwrap();
    service.discard_live_session(&session_id).await.unwrap();
    cleanup.lock().unwrap().take();
    drop(service);
    drop(machine);
    drop(store);
    tokio::time::timeout(Duration::from_secs(10), async {
        while old_machine.strong_count() != 0
            || old_sessions.strong_count() != 0
            || old_store.strong_count() != 0
            || old_review.strong_count() != 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("old execution, database and review owners actually close");

    let mut fresh_prompt = PromptInput::new(FRESH_GOAL, None);
    let fresh_id = fresh_prompt.header.id.clone();
    // A genuinely new factory has no reviewer. The same persisted historical
    // allow and identical call IDs cannot supply a current attempt/permission.
    let (service, machine, store, fresh_sessions) = persistent_service(
        database,
        AgentFactory::minimal().without_provider_auth_persistence(),
        config,
        client,
        tools.clone(),
        authorization(&fresh_id),
    );
    *cleanup.lock().unwrap() = Some((machine.clone(), session_id.clone()));
    let reopened = store
        .load_input_state(&runtime, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(&reopened).unwrap(), frozen_row);
    assert_eq!(
        protected_graph(
            &stored_audit(&reopened),
            &session_id,
            &input_id,
            &reviewer_selection,
            &endpoint,
            &[CALL_A, CALL_B]
        ),
        graph
    );
    assert_eq!(
        store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap()
            .bytes(),
        frozen_document.as_slice()
    );
    let persisted = service
        .load_authoritative_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    let recovered = build_recovered_session(
        persisted.clone(),
        &SurfaceSessionRecoveryOverrides::default(),
        SurfaceSessionRecoveryContext::default(),
    )
    .unwrap();
    materialize(
        &service,
        &machine,
        persisted,
        recovered.into_deferred_create_request(),
    )
    .await;
    meerkat_core::auth::rehydrate_marked_tokens_for_status_for_identity(
        tokens.token_store().as_ref(),
        &machine.generated_auth_lease_handle(),
        selected.credential(),
        meerkat_core::auth::PersistedAuthMode::ApiKey,
        std::time::SystemTime::now().into(),
    )
    .await
    .unwrap()
    .expect("actual current host credential remains available");
    let pin = {
        let lease = service
            .acquire_live_session_actor_turn_boundary_lease(&session_id)
            .await
            .unwrap();
        service
            .pin_controller_client_for_actor(&lease)
            .await
            .unwrap()
    };
    fresh_prompt.header.authority_association =
        Some(association(&runtime, controller, operation, selected));
    let input = Input::Prompt(fresh_prompt);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::super::super::evidence("fresh-post-reopen-review-authentication"),
    )
    .unwrap()
    .with_controller_client(&input, pin)
    .unwrap();
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input.with_ingress_context(current).unwrap())
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        controller_server.receiver.second_request.notified(),
    )
    .await
    .unwrap();
    let bodies = controller_server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 4);
    assert_feedback(&bodies[3], None);
    assert_eq!(
        reviewer_server.receiver.bodies.lock().unwrap().len(),
        2,
        "historical audit does not start another reviewer"
    );
    controller_server.receiver.finish.notify_one();
    let outcome = completion.unwrap().wait().await.unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("missing review remains local: {outcome:?}")
    };
    assert_eq!(result.text, FINISHED);
    assert_eq!(result.session_id, session_id);
    assert!(result.terminal_cause_kind.is_none());
    assert_eq!(
        tools
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.as_str() == allowed_call)
            .count(),
        1
    );
    assert!(!tools.entries.lock().unwrap().contains(&denied_call));
    assert_eq!(
        tools
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.as_str() == SIBLING)
            .count(),
        2
    );
    let fresh = store
        .load_input_state(&runtime, &fresh_id)
        .await
        .unwrap()
        .unwrap();
    let fresh_audit = stored_audit(&fresh);
    assert!(!fresh_audit.iter().any(|record| matches!(
        record.observation.observation,
        AuditObservation::ReviewAttemptStarted { .. }
    )));
    assert!(
        fresh_audit
            .iter()
            .all(|record| record.observation.review_attribution.is_none())
    );
    assert!(successful_tool_outcome(&fresh_audit, SIBLING));
    assert!(!successful_tool_outcome(&fresh_audit, CALL_A));
    assert!(!successful_tool_outcome(&fresh_audit, CALL_B));
    assert_eq!(
        serde_json::to_value(
            store
                .load_input_state(&runtime, &input_id)
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        frozen_row
    );
    let final_document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    assert!(final_document.session().messages().iter().any(
        |message| matches!(message, Message::User(user) if user.text_content() == FRESH_GOAL)
    ));
    machine
        .unregister_current_session_registration_until_terminal(&session_id)
        .await
        .unwrap();
    service.discard_live_session(&session_id).await.unwrap();
    cleanup.lock().unwrap().take();
    let fresh_machine = Arc::downgrade(&machine);
    let fresh_store = Arc::downgrade(&store);
    drop(final_document);
    drop(service);
    drop(machine);
    drop(store);
    tokio::time::timeout(Duration::from_secs(10), async {
        while fresh_machine.strong_count() != 0
            || fresh_store.strong_count() != 0
            || fresh_sessions.strong_count() != 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fresh native and database owners drain before the temporary database is removed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_review_children_keep_exact_candidate_joins_after_sqlite_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("review-correlation.sqlite3");
    let mut controller =
        Server::start_with_tool_responses(vec![concurrent_response(), concurrent_response()]).await;
    let replies = HashMap::from([
        (CALL_A, Arc::new(ControlledReviewReply::default())),
        (CALL_B, Arc::new(ControlledReviewReply::default())),
    ]);
    let mut reviewer = Server::start_with_receiver(Receiver {
        api_key: Some(REVIEW_KEY),
        controlled_review_replies: replies,
        ..Receiver::default()
    })
    .await;
    let cleanup = Cleanup::new(None);
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        exercise(&controller, &reviewer, &database, &cleanup),
    ))
    .catch_unwind()
    .await;
    controller.receiver.finish.notify_one();
    // Release held handlers before native cleanup even after an assertion or
    // timeout. Unreleased response bodies cannot retain the owner being joined.
    for reply in reviewer.receiver.controlled_review_replies.values() {
        reply
            .response
            .lock()
            .unwrap()
            .get_or_insert_with(|| review_response(Case::Deny));
        reply.release.notify_one();
    }
    let retained = cleanup.lock().unwrap().take();
    let drained = if let Some((machine, session)) = retained {
        Some(
            tokio::time::timeout(
                Duration::from_secs(10),
                machine.unregister_current_session_registration_until_terminal(&session),
            )
            .await,
        )
    } else {
        None
    };
    controller.reap().await;
    reviewer.reap().await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("bounded concurrent review/reopen case: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    if let Some(result) = drained {
        result
            .expect("bounded native cleanup")
            .expect("terminal native cleanup");
    }
}
