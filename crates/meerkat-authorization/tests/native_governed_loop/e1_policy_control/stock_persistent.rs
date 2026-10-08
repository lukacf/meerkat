//! One stock persistent service turn over the supplied WholeBlob backend.
//! Reuses E1 HTTP/native owners and reads the real committed Session document.
//! The SQLite reopen case reconstructs an actor from the real committed rows.
//! Persistent controller administration and process restart are separate scopes.
use super::*;
use meerkat::surface::{
    PersistentRuntimeExecutor, SurfaceSessionRecoveryContext, SurfaceSessionRecoveryOverrides,
    build_recovered_session, build_runtime_backed_service_with_default_reconfigure_host,
    materialize_session_with_reserved_admission_and_actor_slot,
};
use meerkat_runtime::accept::AcceptOutcome;
use meerkat_runtime::input_state::{InputLifecycleState, InputTerminalOutcome, StoredInputState};
use meerkat_runtime::{
    InMemoryRuntimeStore, MeerkatDriverKind, RuntimeSessionPersistenceProfile, RuntimeStore,
};

type CleanupSlot = Mutex<Option<(Arc<MeerkatMachine>, SessionId)>>;

#[derive(Clone, Copy)]
enum ControllerProbe {
    TerminalRevocation,
    UnfinishedKeeper,
}

struct StockCredentialHost {
    client: Arc<dyn LlmClient>,
    credential_persistence: meerkat_core::auth::ProviderAuthPersistence,
}

fn stock_association_with_controller_lineage(
    runtime: &LogicalRuntimeId,
    controller_lineage: &[GrantLineageRef],
    operation: GrantLineageRef,
    selected: ControllerModelSelection,
) -> InputAuthorityAssociation {
    let mut candidate = association(
        runtime,
        controller_lineage
            .first()
            .expect("issued controller root")
            .clone(),
        operation,
        selected,
    )
    .candidate()
    .clone();
    candidate.controller_grant_lineage = controller_lineage.to_vec();
    InputAuthorityAssociation::new(candidate).expect("exact issued controller lineage")
}

struct StockPersistentOptions {
    fresh_work_after_reopen: bool,
    notification_probe: bool,
    controller_probe: Option<ControllerProbe>,
}

const REOPEN_DENIED_CALL: &str = "post-reopen-delete";
const REOPEN_PERMITTED_CALL: &str = "post-reopen-read";
const REOPEN_PROMPT: &str = "After SQLite reopen, attempt the two fresh record actions";
const UNAUTHORIZED_NOTIFICATION: &str = "b7-notification-without-source-or-audience-authority";
const NOTIFICATION_MODEL: &str = "claude-opus-4-8";

fn decode_stored_audit(
    row: Value,
) -> Result<Vec<StoredAuthorizationAuditObservation>, serde_json::Error> {
    #[derive(serde::Deserialize)]
    struct AuditProjection {
        // StoredInputState deliberately omits an empty frozen audit prefix.
        // Explicit null or malformed present payloads must still fail decoding.
        #[serde(default)]
        authorization_audit: Vec<StoredAuthorizationAuditObservation>,
    }
    serde_json::from_value::<AuditProjection>(row).map(|row| row.authorization_audit)
}

fn stored_audit(row: &StoredInputState) -> Vec<StoredAuthorizationAuditObservation> {
    decode_stored_audit(serde_json::to_value(row).unwrap()).expect("actual stored row audit")
}

#[test]
fn stored_audit_decoder_accepts_only_valid_arrays_or_omitted_empty_prefix() {
    let empty_row = StoredInputState::new_accepted(meerkat_core::InputId::new());
    let encoded = serde_json::to_value(&empty_row).unwrap();
    assert!(encoded.get("authorization_audit").is_none());
    assert!(decode_stored_audit(encoded).unwrap().is_empty());
    assert!(
        decode_stored_audit(serde_json::json!({"authorization_audit": []}))
            .unwrap()
            .is_empty()
    );

    // A typed historical record exercises decoding only, not native admission.
    let record = StoredAuthorizationAuditObservation {
        contributors: Vec::new().into(),
        observation: meerkat_authorization_contracts::audit::AuthorizationAuditObservation {
            operation_id: meerkat_core::OperationId::new(),
            execution_scope: OperationExecutionScope::Domain,
            run_id: None,
            context_revision: None,
            review_attribution: None,
            observation: AuditObservation::Entry,
        },
    };
    let expected = vec![record];
    assert_eq!(
        decode_stored_audit(serde_json::json!({"authorization_audit": &expected})).unwrap(),
        expected,
    );
    for malformed in [
        serde_json::json!({"authorization_audit": null}),
        serde_json::json!({"authorization_audit": {}}),
        serde_json::json!({"authorization_audit": [null]}),
        serde_json::json!({"authorization_audit": [{}]}),
    ] {
        assert!(decode_stored_audit(malformed).is_err());
    }
}

fn assert_original(
    row: &StoredInputState,
    input_id: &meerkat_core::InputId,
    claims: &InputAuthorityAssociation,
) {
    assert_eq!(&row.state.input_id, input_id);
    assert_eq!(row.state.authority_contributors.len(), 1);
    assert_eq!(row.state.authority_contributors[0].input_id(), input_id);
    assert_eq!(row.state.authority_contributors[0].association(), claims);
}

fn assert_stock_document(saved: &Session, session_id: &SessionId) {
    assert_stock_document_batches(saved, session_id, &[(DENIED_CALL, PERMITTED_CALL)]);
}

fn assert_stock_document_batches(
    saved: &Session,
    session_id: &SessionId,
    batches: &[(&str, &str)],
) {
    assert_eq!(saved.id(), session_id);
    for (denied_call, permitted_call) in batches {
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
                if ids == [*denied_call, *permitted_call] {
                    assert_eq!(results.len(), 2);
                    assert_eq!(
                        results
                            .iter()
                            .filter(|result| result.tool_use_id == *denied_call && result.is_error)
                            .count(),
                        1
                    );
                    assert_eq!(
                        results
                            .iter()
                            .filter(|result| result.tool_use_id == *permitted_call
                                && !result.is_error
                                && result.text_content() == "record-7 value")
                            .count(),
                        1
                    );
                    paired += 1;
                }
            }
        }
        assert_eq!(
            paired, 1,
            "one committed batch preserves these exact siblings"
        );
    }
    assert_eq!(saved.messages().iter().filter(|message| matches!(message,
        Message::BlockAssistant(assistant) if assistant.text_blocks().collect::<String>() == FINISHED)).count(), batches.len());
}

fn assert_stock_turn_audit(
    audit: &[StoredAuthorizationAuditObservation],
    session_id: &SessionId,
    input_id: &meerkat_core::InputId,
    run_id: &meerkat_core::RunId,
    denied_call: &str,
    permitted_call: &str,
) {
    for record in audit {
        assert_eq!(record.observation.run_id.as_ref(), Some(run_id));
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(&record.contributors[0].input_id, input_id);
        assert!(record.contributors[0].requester == principal("requester"));
        assert!(record.contributors[0].logical_executor == principal("executor"));
        assert!(record.contributors[0].represented_subject.is_none());
        assert!(
            matches!(&record.observation.execution_scope, OperationExecutionScope::RuntimeInput {
            owner_session_id, submitted_input_id, canonical_input_id, ..
        } if owner_session_id == session_id && submitted_input_id == input_id && canonical_input_id == input_id)
        );
    }
    let refused: Vec<_> = audit
        .iter()
        .filter(|record| {
            matches!(&record.observation.observation,
        AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
            if call_id == denied_call && tool_name == "delete_record"))
        })
        .collect();
    assert_eq!(refused.len(), 1);
    assert!(!audit.iter().any(|record| record.observation.operation_id
        == refused[0].observation.operation_id
        && matches!(
            record.observation.observation,
            AuditObservation::Prepared { .. }
                | AuditObservation::Entry
                | AuditObservation::Outcome { .. }
        )));
    let prepared: Vec<_> = audit
        .iter()
        .filter(|record| {
            matches!(&record.observation.observation,
        AuditObservation::Prepared { target, .. }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
            if call_id == permitted_call && tool_name == "read_record"))
        })
        .collect();
    assert_eq!(prepared.len(), 1);
    let read_id = &prepared[0].observation.operation_id;
    let read_sequence: Vec<_> = audit
        .iter()
        .filter(|record| &record.observation.operation_id == read_id)
        .map(|record| &record.observation.observation)
        .collect();
    assert_eq!(
        read_sequence.len(),
        3,
        "one exact entered tool audit sequence"
    );
    assert!(matches!(
        read_sequence[0],
        AuditObservation::Prepared { .. }
    ));
    assert!(matches!(read_sequence[1], AuditObservation::Entry));
    assert!(matches!(
        read_sequence[2],
        AuditObservation::Outcome {
            outcome: OperationObservedOutcome::ToolDispatchReturned {
                result_is_error: false,
                terminal_error: None,
                ..
            }
        }
    ));
    let models: Vec<_> = audit
        .iter()
        .filter_map(|record| match &record.observation.observation {
            AuditObservation::Prepared { target, .. }
                if matches!(target.as_ref(), AuditTarget::Model(_)) =>
            {
                Some(&record.observation.operation_id)
            }
            _ => None,
        })
        .collect();
    assert_eq!(models.len(), 2);
    for operation_id in models {
        let sequence: Vec<_> = audit
            .iter()
            .filter(|record| &record.observation.operation_id == operation_id)
            .map(|record| &record.observation.observation)
            .collect();
        assert_eq!(sequence.len(), 3, "one exact entered model audit sequence");
        assert!(matches!(sequence[0], AuditObservation::Prepared { .. }));
        assert!(matches!(sequence[1], AuditObservation::Entry));
        assert!(matches!(
            sequence[2],
            AuditObservation::Outcome {
                outcome: OperationObservedOutcome::HttpResponse { status: 200 }
            }
        ));
    }
}

async fn exercise_stock_persistent(
    server: &Server,
    cleanup: &CleanupSlot,
    session_store: Arc<dyn meerkat::SessionStore>,
    store: Arc<dyn RuntimeStore>,
    reopen_database: Option<&std::path::Path>,
    options: StockPersistentOptions,
    host: StockCredentialHost,
) {
    let StockPersistentOptions {
        fresh_work_after_reopen,
        notification_probe,
        controller_probe,
    } = options;
    assert!(
        !notification_probe || (fresh_work_after_reopen && reopen_database.is_some()),
        "the notification prerequisite includes real reopen and fresh permitted work"
    );
    let StockCredentialHost {
        client,
        credential_persistence,
    } = host;
    let old_session_store = Arc::downgrade(&session_store);
    let old_runtime_store = Arc::downgrade(&store);
    let selected = client
        .controller_model_selection()
        .expect("actual HTTP selection");
    assert_eq!(selected.model(), server.model());
    let publication = LocalAuthorizationPublication::new();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("stock-persistent-native-grants"),
                generation: 1,
            },
            publication.clone(),
            Arc::new(HostAuthorizationClock),
        )
        .unwrap(),
    );
    // Only the two administration probes exercise delegated controllers.
    // All existing stock and cold-process controls retain their root lineage.
    let delegated_controller = controller_probe.is_some();
    let mut controller_bounds = ceiling("infer");
    if delegated_controller {
        controller_bounds.delegation_depth =
            meerkat_authorization_contracts::constraints::DelegationDepth::remaining(1);
    }
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal(if delegated_controller {
                "controller-delegator"
            } else {
                "executor"
            }),
            None,
            controller_bounds,
        )
        .unwrap();
    let controller_lineage = if delegated_controller {
        let mut leaf_bounds = ceiling("infer");
        leaf_bounds.delegation_depth =
            meerkat_authorization_contracts::constraints::DelegationDepth::remaining(0);
        let leaf = grants
            .issue_child(
                &principal("controller-delegator"),
                &controller,
                id("controller-leaf"),
                principal("executor"),
                leaf_bounds,
            )
            .expect("actual issued controller child");
        vec![controller.clone(), leaf]
    } else {
        vec![controller.clone()]
    };
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .unwrap();
    let ingress_controller_lineage = controller_lineage.clone();
    let ingress_operation = operation.clone();
    let ingress_selection = selected.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("native-loop").unwrap()
            || claimed
                != &stock_association_with_controller_lineage(
                    runtime,
                    &ingress_controller_lineage,
                    ingress_operation.clone(),
                    ingress_selection.clone(),
                )
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let http_owner = Arc::new(HttpRecordOwner {
        selection: selected.clone(),
        endpoint: format!("{}/v1/messages", server.base_url),
    });
    let context_owner = notification_probe
        .then(|| context_append::ControlOwner::new(http_owner.clone(), publication));
    let invocation_owner: Arc<dyn AdmittedWorkPolicyOwner> = if let Some(owner) = &context_owner {
        owner.clone()
    } else {
        Arc::new(InvocationOwner)
    };
    let operation_owner: Arc<dyn OperationPolicyOwner> = if let Some(owner) = &context_owner {
        owner.clone()
    } else {
        http_owner
    };
    let bundle = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        session_store,
        store.clone(),
        Arc::new(meerkat::MemoryBlobStore::new()),
        NativeGrantWorkConfiguration {
            grants: grants.clone(),
            ingress: ingress.clone(),
            invocation_owner: invocation_owner.clone(),
            operation_owner: operation_owner.clone(),
        },
    )
    .expect("WholeBlob backend configures the bundle's actual governed persistent owner");
    assert_eq!(
        bundle.session_persistence_profile(),
        RuntimeSessionPersistenceProfile::WholeBlobV1
    );
    assert!(Arc::ptr_eq(&bundle.runtime_store(), &store));
    let notification_job_store = notification_probe.then(|| bundle.job_store());
    let configured_adapter = bundle.runtime_adapter();
    let tools = Arc::new(RecordingTools::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(client.clone());
    builder.default_tool_dispatcher = Some(tools.clone());
    // The stock builder installs the bundle's StoreAdapter and blob store.
    // A fresh unused path only configures the stock reconfigure host; this test
    // never executes reconfiguration or reads/writes a configuration file.
    let config_path =
        std::env::temp_dir().join(format!("stock-persistent-{}.toml", SessionId::new()));
    let (service, machine) =
        build_runtime_backed_service_with_default_reconfigure_host(builder, 2, bundle, config_path);
    assert!(Arc::ptr_eq(&machine, &configured_adapter));
    assert!(Arc::ptr_eq(&service.runtime_store(), &store));
    let seed = Session::new();
    let session_id = seed.id().clone();
    *cleanup.lock().unwrap() = Some((machine.clone(), session_id.clone()));
    let reserved = service.reserve_create_session_admission().await.unwrap();
    let created = Box::pin(materialize_session_with_reserved_admission_and_actor_slot(
        &service,
        &machine,
        seed,
        CreateSessionRequest {
            model: server.model().into(),
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
                        .expect("actual selected binding")
                        .clone(),
                ),
                ..Default::default()
            }),
            labels: None,
        },
        reserved,
        {
            let service = service.clone();
            let machine = machine.clone();
            move |session_id, _attachment, actor_slot| {
                Box::new(
                    PersistentRuntimeExecutor::new(service, machine, session_id)
                        .with_publication_actor_slot(actor_slot),
                )
            }
        },
    ))
    .await
    .expect("stock persistent service and real actor-slot executor");
    assert_eq!(created.session_id, session_id);
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let snapshot = machine
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .unwrap();
    assert_eq!(snapshot.binding.driver_kind, MeerkatDriverKind::Persistent);
    assert_eq!(snapshot.binding.runtime_id, runtime);
    assert!(store.load_input_states(&runtime).await.unwrap().is_empty());
    assert!(server.receiver.bodies.lock().unwrap().is_empty());
    let initial_document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .expect("stock service committed its initial Session document");
    assert_eq!(initial_document.session().id(), &session_id);
    let initial_bytes = initial_document.bytes().to_vec();
    let initial_revision = initial_document.authority().store_revision();
    assert!(initial_revision > 0);
    assert!(tool_feedback(initial_document.session().messages(), DENIED_CALL).is_none());
    assert!(tool_feedback(initial_document.session().messages(), PERMITTED_CALL).is_none());

    let pin = {
        let actor_lease = service
            .acquire_live_session_actor_turn_boundary_lease(&session_id)
            .await
            .expect("current stock persistent actor under its turn boundary");
        service
            .pin_controller_client_for_actor(&actor_lease)
            .await
            .unwrap()
    };
    assert!(
        pin.selection() == &selected,
        "real actor keeps the actual HTTP client"
    );
    let published_tokens = meerkat_auth_core::save_tokens_and_publish_lifecycle(
        credential_persistence.clone(),
        machine.generated_auth_lease_handle(),
        selected.credential().clone(),
        meerkat_core::auth::PersistedTokens::api_key("synthetic-e1-loopback-only"),
    )
    .await
    .expect("same native generated credential owner");
    let claims = stock_association_with_controller_lineage(
        &runtime,
        &controller_lineage,
        operation.clone(),
        selected.clone(),
    );
    let mut prompt = PromptInput::new("Attempt the two record actions", None);
    prompt.header.authority_association = Some(claims.clone());
    let input = Input::Prompt(prompt);
    let input_id = input.id().clone();
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::super::evidence("stock-persistent-current-authentication"),
    )
    .unwrap()
    .with_controller_client(&input, pin)
    .unwrap();
    let (accepted, completion) = machine
        .accept_input_with_completion(&session_id, input.with_ingress_context(current).unwrap())
        .await
        .expect("actual persistent native admission");
    assert!(
        matches!(accepted, AcceptOutcome::Accepted { input_id: ref accepted_id, .. }
        if accepted_id == &input_id)
    );
    let completion = completion.expect("actual native completion handle");
    tokio::time::timeout(
        Duration::from_secs(20),
        server.receiver.second_request.notified(),
    )
    .await
    .expect("both sibling results reach actual second HTTP request");
    let bodies = server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2);
    for body in &bodies {
        assert_eq!(body["model"], server.model());
        assert_eq!(body["stream"], true);
    }
    assert_wire_sibling_feedback(&bodies[1]);
    assert_eq!(*tools.0.lock().unwrap(), ["read_record"]);

    let live = machine
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    let live_audit = stored_audit(&live);
    assert!(
        !live_audit.is_empty(),
        "live native observations must be present before completion"
    );
    let run_id = live_audit
        .first()
        .unwrap()
        .observation
        .run_id
        .clone()
        .unwrap();
    let snapshot = machine
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .unwrap();
    assert_eq!(snapshot.binding.driver_kind, MeerkatDriverKind::Persistent);
    assert_eq!(snapshot.control.current_run_id.as_ref(), Some(&run_id));
    // This is the backend row, not the live machine query or a test-owned copy.
    let before_finish = store
        .load_input_state(&runtime, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert_original(&before_finish, &input_id, &claims);
    assert_eq!(before_finish.seed.last_run_id.as_ref(), Some(&run_id));
    assert_eq!(
        before_finish
            .state
            .persisted_input
            .as_ref()
            .unwrap()
            .header()
            .authority_association
            .as_ref(),
        Some(&claims)
    );
    let frozen_before = serde_json::to_value(&before_finish).unwrap();
    let prior_audit = stored_audit(&before_finish);
    assert!(live_audit.starts_with(&prior_audit));

    // Exercise the real default Event sink while the second model response is
    // held. Job lineage is not native ingress authority. This covers the sink
    // and admission boundary, not inbox retry/acknowledgement ownership.
    {
        use meerkat::JobDeliverySink;

        let document_before_delivery = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap();
        let job_id = meerkat::JobId::new("stock-governed-event").unwrap();
        let subscription = meerkat::JobSubscription::new(
            meerkat::JobSubscriptionId::new("unauthenticated-event").unwrap(),
            session_id.clone(),
            meerkat::JobDeliveryKind::Event {
                handling_mode: meerkat_core::HandlingMode::Queue,
            },
        );
        let delivery_key = format!("job:{job_id}:1:unauthenticated-event");
        let sink =
            meerkat::surface::SessionServiceDeliverySink::new(service.clone(), machine.clone());
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            sink.apply(meerkat::JobDeliveryApplication::Event {
                job_id,
                delivery_sequence: 1,
                subscription,
                interaction_lineage_id: meerkat::InteractionLineageId::new(),
                handling_mode: meerkat_core::HandlingMode::Queue,
                content: meerkat::JobDeliveryContent::Terminal(
                    meerkat::JobTerminalResult::Succeeded { result_ref: None },
                ),
            }),
        )
        .await
        .expect("missing ingress is rejected without waiting for the held turn")
        .expect_err("the default Event producer cannot mint governed authority");
        assert_eq!(
            error,
            meerkat::JobDeliveryApplyError::Infrastructure(
                meerkat_runtime::RuntimeDriverError::ValidationFailed {
                    reason: "native work authority association is unavailable or mismatched".into(),
                }
                .to_string(),
            ),
        );
        assert!(
            machine
                .input_state_by_idempotency_key(&session_id, &delivery_key)
                .await
                .unwrap()
                .is_none()
        );
        let rows = store.load_input_states_strict(&runtime).await.unwrap();
        assert_eq!(rows.len(), 1, "the delivery creates no durable input");
        assert_eq!(serde_json::to_value(&rows[0]).unwrap(), frozen_before);
        assert_eq!(
            serde_json::to_value(machine.input_state(&session_id, &input_id).await.unwrap())
                .unwrap(),
            serde_json::to_value(Some(&live)).unwrap(),
            "failed delivery does not alter the admitted original or its audit",
        );
        let document_after_delivery = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            document_after_delivery.bytes(),
            document_before_delivery.bytes()
        );
        assert_eq!(
            document_after_delivery.authority(),
            document_before_delivery.authority(),
        );
        assert_eq!(*server.receiver.bodies.lock().unwrap(), bodies);
        assert_eq!(
            server.receiver.authorized_requests.load(Ordering::SeqCst),
            2
        );
        assert_eq!(*tools.0.lock().unwrap(), ["read_record"]);
    }

    if matches!(controller_probe, Some(ControllerProbe::UnfinishedKeeper)) {
        // The real second model response is still held. The actual stored
        // contributor and current run, not a mirror counter, retain the grant.
        assert_eq!(controller_lineage.len(), 2);
        assert_eq!(
            claims.candidate().controller_grant_lineage,
            controller_lineage
        );
        let resolved = grants
            .resolve_controller_lineage(&controller_lineage, &principal("executor"), None)
            .unwrap();
        let token_store = credential_persistence.token_store();
        let token_key =
            meerkat_core::auth::TokenKey::from_credential_identity(selected.credential());
        let frozen_tokens = token_store.load(&token_key).await.unwrap();
        assert_eq!(frozen_tokens.as_ref(), Some(&published_tokens));
        let lease_key =
            meerkat_core::handles::LeaseKey::from_credential_identity(selected.credential());
        let frozen_lease = machine.generated_auth_lease_handle().snapshot(&lease_key);
        let frozen_document = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap();
        {
            let mut custody = machine
                .try_controller_grant_mutation()
                .expect("persistent owner must inspect its actual unfinished contributor");
            for reference in &controller_lineage {
                assert_eq!(
                    grants.revoke(&principal("grant-owner"), reference, &mut custody),
                    Err(meerkat_authorization::grants::GrantRefusal::ControllerInUse),
                    "the actual unfinished contributor retains both its ancestor and leaf"
                );
            }
        }
        assert_eq!(
            resolved.check_current(),
            Ok(()),
            "refusal cannot publish grant changes"
        );
        grants
            .resolve_controller_lineage(&controller_lineage, &principal("executor"), None)
            .expect("retained full controller lineage remains usable");
        assert_eq!(token_store.load(&token_key).await.unwrap(), frozen_tokens);
        assert_eq!(
            machine.generated_auth_lease_handle().snapshot(&lease_key),
            frozen_lease
        );
        assert_eq!(
            serde_json::to_value(
                store
                    .load_input_state(&runtime, &input_id)
                    .await
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            frozen_before
        );
        let unchanged = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.bytes(), frozen_document.bytes());
        assert_eq!(unchanged.authority(), frozen_document.authority());
        assert_eq!(*server.receiver.bodies.lock().unwrap(), bodies);
        assert_eq!(
            server.receiver.authorized_requests.load(Ordering::SeqCst),
            2
        );
        assert_eq!(*tools.0.lock().unwrap(), ["read_record"]);
    }

    server.receiver.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("ordinary refusal must not terminate persistent work: {outcome:?}")
    };
    assert_eq!(result.text, FINISHED);
    assert_eq!(result.session_id, session_id);
    assert!(result.terminal_cause_kind.is_none());
    assert_eq!(server.receiver.bodies.lock().unwrap().len(), 2);
    assert_eq!(
        server.receiver.authorized_requests.load(Ordering::SeqCst),
        2
    );
    assert_eq!(*tools.0.lock().unwrap(), ["read_record"]);
    // Read the actual committed document/identity pair from the runtime backend,
    // independently of the live actor and SessionStore compatibility projection.
    let final_document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .expect("actual committed Session document after native completion");
    assert_eq!(final_document.session().id(), &session_id);
    assert_eq!(final_document.authority().session_id(), &session_id);
    assert!(final_document.authority().store_revision() > initial_revision);
    assert_ne!(final_document.bytes(), initial_bytes.as_slice());
    assert_eq!(
        initial_document.bytes(),
        initial_bytes.as_slice(),
        "old committed bytes are immutable"
    );
    let decoded = Session::decode_whole_blob_document(final_document.bytes()).unwrap();
    assert_eq!(
        decoded.row_sha256_token(),
        final_document.authority().blob_sha256()
    );
    let saved = decoded.into_session();
    assert_stock_document(&saved, &session_id);
    let authoritative = service
        .load_authoritative_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_stock_document(&authoritative, &session_id);
    assert_eq!(
        serde_json::to_value(authoritative.messages()).unwrap(),
        serde_json::to_value(saved.messages()).unwrap(),
        "stock service reads the committed transcript"
    );
    let refusal = tool_feedback(saved.messages(), DENIED_CALL).unwrap();
    assert!(refusal.is_error);
    assert_eq!(
        serde_json::from_str::<Value>(&refusal.text_content()).unwrap(),
        ToolError::AuthorizationRefused { refusal: denied() }.to_error_payload()
    );
    let allowed = tool_feedback(saved.messages(), PERMITTED_CALL).unwrap();
    assert!(!allowed.is_error);
    assert_eq!(allowed.text_content(), "record-7 value");

    // Completion must make the actual store's frozen audit prefix current.
    let final_row = store
        .load_input_state(&runtime, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert_original(&final_row, &input_id, &claims);
    assert_eq!(final_row.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(
        final_row.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed)
    );
    assert_eq!(final_row.seed.last_run_id.as_ref(), Some(&run_id));
    assert_eq!(
        serde_json::to_value(&before_finish).unwrap(),
        frozen_before,
        "backend read must not alias later live audit appends"
    );
    let audit = stored_audit(&final_row);
    assert!(
        !audit.is_empty(),
        "terminal durable audit must be present after completion"
    );
    assert!(
        audit.starts_with(&live_audit),
        "terminal store retains observed audit order"
    );
    assert!(
        audit.len() > prior_audit.len(),
        "terminal commit must persist later observations"
    );
    assert_stock_turn_audit(
        &audit,
        &session_id,
        &input_id,
        &run_id,
        DENIED_CALL,
        PERMITTED_CALL,
    );
    let mut expected_bytes = final_document.bytes().to_vec();
    let mut expected_authority = final_document.authority().clone();
    let mut expected_messages = serde_json::to_value(final_document.session().messages()).unwrap();
    if notification_probe {
        use meerkat::JobDeliverySink;

        // The original turn has completed and committed its exact sibling/audit
        // results. Do not mistake an append waiting behind a held response for
        // an authorization refusal: establish the idle actor, then await the
        // actual sink result before examining either live or stored context.
        let idle_actor = service
            .acquire_live_session_actor_turn_boundary_lease(&session_id)
            .await
            .expect("known live idle actor after native completion");
        drop(idle_actor);
        let live_before = service
            .export_live_session(&session_id)
            .await
            .expect("the idle actor remains the live transcript owner");
        assert_eq!(
            serde_json::to_value(live_before.messages()).unwrap(),
            serde_json::to_value(saved.messages()).unwrap(),
        );
        let original_live_row = machine
            .input_state(&session_id, &input_id)
            .await
            .unwrap()
            .expect("the completed original remains queryable");
        let job_store = notification_job_store.expect("the actual bundle's job store");
        let jobs = meerkat::DetachedJobService::new(job_store.clone());
        let submitted = jobs
            .submit(meerkat::JobSpec::new(
                "native-loop",
                session_id.clone(),
                meerkat::ExecutionIntentId::new(),
                meerkat::InteractionLineageId::new(),
                meerkat::ToolIdentity::new("stock-notification", "1").unwrap(),
                meerkat::RunnerIdentity::new("stock-notification", "1").unwrap(),
                meerkat::RestartClass::Adoptable,
                meerkat::CanonicalArgumentsHash::new("stock-notification-arguments").unwrap(),
                meerkat::JobSubmissionKey::new("stock-notification-submission").unwrap(),
            ))
            .await
            .expect("commit a real job for the known session");
        assert!(!submitted.deduplicated);
        let job_id = submitted.job_id;
        let subscription = meerkat::JobSubscription::new(
            meerkat::JobSubscriptionId::new("unauthenticated-notification").unwrap(),
            session_id.clone(),
            meerkat::JobDeliveryKind::Notification,
        );
        jobs.subscribe(&job_id, subscription.clone())
            .await
            .expect("commit the Notification subscription through its job owner");
        let stored_job = job_store
            .get(&job_id)
            .await
            .unwrap()
            .expect("the job exists before invoking the default sink");
        assert_eq!(stored_job.spec.origin_session_id, session_id);
        assert_eq!(stored_job.spec.realm_id, "native-loop");
        assert_eq!(stored_job.subscriptions, vec![subscription.clone()]);
        let content = meerkat::JobDeliveryContent::Notification(
            meerkat::JobNotification::new(
                "stock-notification",
                "stock-notification-once",
                "Notification without governing authority",
                UNAUTHORIZED_NOTIFICATION,
            )
            .expect("valid nonempty notification body and identifiers"),
        );
        let request =
            meerkat::job_delivery_notification_request(&job_id, 1, &subscription, &content);
        assert!(request.text().contains(UNAUTHORIZED_NOTIFICATION));
        assert_eq!(request.source, Some(format!("detached_job:{job_id}")));
        assert_eq!(
            request.idempotency_key,
            Some(format!("job:{job_id}:1:unauthenticated-notification"))
        );
        // Job identity and its target session supply neither authenticated
        // ingress nor source/audience entitlement. This is the real default
        // sink, with no replacement policy, grant or admission implementation.
        let sink =
            meerkat::surface::SessionServiceDeliverySink::new(service.clone(), machine.clone());
        let delivered = tokio::time::timeout(
            Duration::from_secs(5),
            sink.apply(meerkat::JobDeliveryApplication::Notification {
                job_id: job_id.clone(),
                delivery_sequence: 1,
                subscription,
                content,
            }),
        )
        .await
        .expect("idle notification application must settle; a timeout is not a refusal");
        let document_after = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !serde_json::to_string(document_after.session().messages())
                .unwrap()
                .contains(UNAUTHORIZED_NOTIFICATION),
            "notification without governing authority entered the stored model context"
        );
        assert_eq!(
            serde_json::to_value(document_after.session().messages()).unwrap(),
            serde_json::to_value(final_document.session().messages()).unwrap(),
            "control refusal changes protected audit, not the transcript"
        );
        let controls_before = final_document
            .session()
            .context_control_observations()
            .collect::<Result<Vec<_>, _>>()
            .expect("existing typed control audit");
        let controls_after = document_after
            .session()
            .context_control_observations()
            .collect::<Result<Vec<_>, _>>()
            .expect("settled typed control audit");
        assert_eq!(controls_after.len(), controls_before.len() + 1);
        assert!(
            controls_before
                .iter()
                .all(|old| controls_after.contains(old))
        );
        let new_controls: Vec<_> = controls_after
            .iter()
            .filter(|record| !controls_before.contains(record))
            .collect();
        assert_eq!(new_controls.len(), 1);
        let observation = new_controls[0];
        assert_eq!(observation.session_id, session_id);
        assert!(observation.requester.is_none());
        assert!(observation.actor.is_none());
        assert!(observation.realm.is_none());
        assert!(observation.source.is_none());
        assert!(observation.policy.is_empty());
        assert_eq!(
            observation.request_digest,
            meerkat_core::session::context_control::ContextControlAuditRecord::digest_request(
                &request,
            )
            .unwrap(),
        );
        assert_eq!(
            observation.outcome,
            meerkat_core::session::context_control::ContextControlAuditOutcome::AuthorizationUnavailable,
        );
        assert_ne!(document_after.bytes(), final_document.bytes());
        assert_ne!(document_after.authority(), final_document.authority());
        let live_after = service
            .export_live_session(&session_id)
            .await
            .expect("refused notification leaves the live actor available");
        assert_eq!(
            serde_json::to_value(live_after.messages()).unwrap(),
            serde_json::to_value(live_before.messages()).unwrap(),
            "refusal must not leave an unpersisted System append in the live actor"
        );
        assert_eq!(
            live_after
                .context_control_observation(&observation.operation_id)
                .unwrap()
                .as_ref(),
            Some(observation),
            "live actor and committed snapshot retain the same settled control observation"
        );
        let rows_after = store.load_input_states_strict(&runtime).await.unwrap();
        assert_eq!(rows_after.len(), 1, "notification creates no native input");
        assert_eq!(
            serde_json::to_value(&rows_after[0]).unwrap(),
            serde_json::to_value(&final_row).unwrap(),
        );
        assert_eq!(stored_audit(&rows_after[0]), audit);
        assert_eq!(
            serde_json::to_value(machine.input_state(&session_id, &input_id).await.unwrap())
                .unwrap(),
            serde_json::to_value(Some(&original_live_row)).unwrap(),
        );
        assert_eq!(*server.receiver.bodies.lock().unwrap(), bodies);
        assert_eq!(
            server.receiver.authorized_requests.load(Ordering::SeqCst),
            2
        );
        assert_eq!(*tools.0.lock().unwrap(), ["read_record"]);
        assert_eq!(job_store.get(&job_id).await.unwrap(), Some(stored_job));
        assert_eq!(
            delivered,
            Err(meerkat::JobDeliveryApplyError::Authorization(
                meerkat_core::OperationAuthorizationError::Unavailable,
            )),
            "missing authority is typed unavailable, not Busy, NotFound, or a policy verdict"
        );
        context_append::exercise(
            context_owner.as_ref().unwrap(),
            &service,
            &machine,
            &store,
            &session_id,
        )
        .await;
        let settled_controls = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap();
        expected_bytes = settled_controls.bytes().to_vec();
        expected_authority = settled_controls.authority().clone();
        expected_messages = serde_json::to_value(settled_controls.session().messages()).unwrap();
    }
    let Some(database) = reopen_database else {
        return;
    };
    let expected_row = serde_json::to_value(&final_row).unwrap();
    let old_machine = Arc::downgrade(&machine);

    // Close the actual original owners, not just another database facade.
    // The outer cleanup slot remains available until teardown succeeds. Join
    // the owned saga until terminal: the plain unregister's bounded caller
    // grace answers a saga still running under load with typed
    // `UnregisterInProgress`, which is not a failed teardown.
    machine
        .unregister_current_session_registration_until_terminal(&session_id)
        .await
        .expect("original persistent worker reaches terminal teardown");
    service
        .discard_live_session(&session_id)
        .await
        .expect("original stock actor and its exported capabilities are discarded");
    let retained = cleanup.lock().unwrap().take().unwrap();
    assert!(Arc::ptr_eq(&retained.0, &machine));
    assert_eq!(retained.1, session_id);
    drop(retained);
    drop(service);
    drop(machine);
    drop(configured_adapter);
    drop(store);
    // Teardown reports terminality before its supervisor drops its last
    // cloned owner. Wait for actual release, without retaining a polling Arc.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let closed = old_machine.upgrade().is_none()
                && old_session_store.upgrade().is_none()
                && old_runtime_store.upgrade().is_none();
            if closed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|error| {
        panic!(
            "original owners remain after teardown: machine={}, session_store={}, runtime_store={}: {error}",
            old_machine.strong_count(),
            old_session_store.strong_count(),
            old_runtime_store.strong_count(),
        )
    });

    let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
        meerkat::SqliteSessionStore::open(database.to_path_buf())
            .expect("actual SQLite session backend reopens"),
    );
    let store: Arc<dyn RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_whole_blob(database.to_path_buf())
            .expect("actual SQLite WholeBlob runtime backend reopens"),
    );
    let reopened_row = store
        .load_input_state(&runtime, &input_id)
        .await
        .unwrap()
        .expect("terminal input survives physical close/reopen");
    assert_original(&reopened_row, &input_id, &claims);
    assert_eq!(serde_json::to_value(&reopened_row).unwrap(), expected_row);
    assert_eq!(stored_audit(&reopened_row), audit);
    let reopened_document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .expect("committed Session survives physical close/reopen");
    assert_eq!(reopened_document.bytes(), expected_bytes.as_slice());
    assert_eq!(reopened_document.authority(), &expected_authority);
    assert_stock_document(reopened_document.session(), &session_id);

    // These are the same actual trusted host owners. Historical associations
    // supply expected audit data, never replacement admission authority.
    let bundle = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        session_store,
        store.clone(),
        Arc::new(meerkat::MemoryBlobStore::new()),
        NativeGrantWorkConfiguration {
            grants: grants.clone(),
            ingress,
            invocation_owner,
            operation_owner,
        },
    )
    .expect("reopened backend acquires fresh actual governed execution custody");
    let configured_adapter = bundle.runtime_adapter();
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(client);
    builder.default_tool_dispatcher = Some(tools.clone());
    let config_path =
        std::env::temp_dir().join(format!("stock-reopened-{}.toml", SessionId::new()));
    let (service, machine) =
        build_runtime_backed_service_with_default_reconfigure_host(builder, 2, bundle, config_path);
    assert!(Arc::ptr_eq(&machine, &configured_adapter));
    assert!(Arc::ptr_eq(&service.runtime_store(), &store));
    *cleanup.lock().unwrap() = Some((machine.clone(), session_id.clone()));
    if matches!(controller_probe, Some(ControllerProbe::TerminalRevocation)) {
        assert!(
            machine
                .meerkat_machine_spine_snapshot(&session_id)
                .await
                .is_none(),
            "completed history is inspected before any recovered actor is materialized"
        );
        let frozen_row = serde_json::to_value(&reopened_row).unwrap();
        let token_store = credential_persistence.token_store();
        let token_key =
            meerkat_core::auth::TokenKey::from_credential_identity(selected.credential());
        let frozen_tokens = token_store.load(&token_key).await.unwrap();
        assert_eq!(frozen_tokens.as_ref(), Some(&published_tokens));
        let lease_key =
            meerkat_core::handles::LeaseKey::from_credential_identity(selected.credential());
        let frozen_lease = machine.generated_auth_lease_handle().snapshot(&lease_key);
        // The original worker, actor and backend have actually closed. This
        // new empty-map machine owns the reopened backend and the same grants
        // Arc; its own current lease and token snapshots above are the baseline.
        let retained = cleanup.lock().unwrap().take().unwrap();
        assert!(Arc::ptr_eq(&retained.0, &machine));
        assert_eq!(retained.1, session_id);
        drop(retained);
        assert_eq!(controller_lineage.len(), 2);
        // Revoke the leaf first so the still-unrevoked ancestor can independently
        // resolve before its own mutation. Neither retained historical reference
        // is an unfinished-work veto after the actual native run has completed.
        for index in (0..controller_lineage.len()).rev() {
            let executor = principal(if index == 0 {
                "controller-delegator"
            } else {
                "executor"
            });
            let lineage = &controller_lineage[..=index];
            let resolved = grants
                .resolve_controller_lineage(lineage, &executor, None)
                .unwrap();
            {
                let mut custody = machine.try_controller_grant_mutation().expect(
                    "reopened completed history must support native controller maintenance",
                );
                assert_eq!(
                    grants.revoke(
                        &principal("intruder"),
                        &controller_lineage[index],
                        &mut custody
                    ),
                    Err(meerkat_authorization::grants::GrantRefusal::Denied),
                    "entered grant-owner refusal must not become custody Unavailable or success"
                );
                assert_eq!(
                    resolved.check_current(),
                    Ok(()),
                    "rejected grant mutation must not publish"
                );
                grants
                    .revoke(
                        &principal("grant-owner"),
                        &controller_lineage[index],
                        &mut custody,
                    )
                    .expect("the completed run no longer retains this controller reference");
            }
            assert_eq!(
                resolved.check_current(),
                Err(meerkat_authorization::publication::PublicationError::Changed)
            );
            assert!(matches!(
                grants.resolve_controller_lineage(lineage, &executor, None),
                Err(meerkat_authorization::grants::GrantRefusal::Denied)
            ));
        }
        assert_eq!(token_store.load(&token_key).await.unwrap(), frozen_tokens);
        assert_eq!(
            machine.generated_auth_lease_handle().snapshot(&lease_key),
            frozen_lease
        );
        let unchanged_row = store
            .load_input_state(&runtime, &input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_value(&unchanged_row).unwrap(), frozen_row);
        assert_eq!(stored_audit(&unchanged_row), audit);
        let unchanged = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged.bytes(), reopened_document.bytes());
        assert_eq!(unchanged.authority(), reopened_document.authority());
        assert_eq!(*server.receiver.bodies.lock().unwrap(), bodies);
        assert_eq!(
            server.receiver.authorized_requests.load(Ordering::SeqCst),
            2
        );
        assert_eq!(*tools.0.lock().unwrap(), ["read_record"]);
        return;
    }

    drop(
        machine
            .try_controller_grant_mutation()
            .expect("reopened persistent store supports controller custody acquisition"),
    );
    let persisted = service
        .load_authoritative_session(&session_id)
        .await
        .unwrap()
        .expect("stock recovery reads the actual reopened Session");
    assert_stock_document(&persisted, &session_id);
    let recovered = build_recovered_session(
        persisted.clone(),
        &SurfaceSessionRecoveryOverrides::default(),
        SurfaceSessionRecoveryContext::default(),
    )
    .expect("existing owner resolves persisted session build facts");
    let reserved = service.reserve_create_session_admission().await.unwrap();
    let created = Box::pin(materialize_session_with_reserved_admission_and_actor_slot(
        &service,
        &machine,
        persisted,
        recovered.into_deferred_create_request(),
        reserved,
        {
            let service = service.clone();
            let machine = machine.clone();
            move |session_id, _attachment, actor_slot| {
                Box::new(
                    PersistentRuntimeExecutor::new(service, machine, session_id)
                        .with_publication_actor_slot(actor_slot),
                )
            }
        },
    ))
    .await
    .expect("actual stock governed actor reconstructs from reopened stores");
    assert_eq!(created.session_id, session_id);
    let snapshot = machine
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .unwrap();
    assert_eq!(snapshot.binding.driver_kind, MeerkatDriverKind::Persistent);
    assert_eq!(snapshot.binding.runtime_id, runtime);
    let pin = {
        let actor_lease = service
            .acquire_live_session_actor_turn_boundary_lease(&session_id)
            .await
            .expect("newly reconstructed actor has its real turn boundary");
        service
            .pin_controller_client_for_actor(&actor_lease)
            .await
            .unwrap()
    };
    assert!(
        pin.selection() == &selected,
        "fresh actor pin names the actual client"
    );
    let pin = fresh_work_after_reopen.then_some(pin);
    let recovered_row = machine
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .expect("reconstructed native owner reads the protected terminal row");
    assert_eq!(serde_json::to_value(&recovered_row).unwrap(), expected_row);
    assert_eq!(stored_audit(&recovered_row), audit);
    let recovered_document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    assert_stock_document(recovered_document.session(), &session_id);
    assert_eq!(
        serde_json::to_value(recovered_document.session().messages()).unwrap(),
        expected_messages
    );
    let mut fresh_terminal = None;
    if fresh_work_after_reopen {
        // Admission authority comes from the still-current host owners, not the
        // decoded old row. Keep the actual host token owner across this
        // same-process reopen and verify its original marked publication.
        // A new empty token store would instead request release of that live
        // credential, which persistent controller custody currently refuses.
        assert_eq!(server.receiver.bodies.lock().unwrap().len(), 2);
        assert_eq!(*tools.0.lock().unwrap(), ["read_record"]);
        let auth_owner = machine.generated_auth_lease_handle();
        let lease_key =
            meerkat_core::handles::LeaseKey::from_credential_identity(selected.credential());
        let before = auth_owner.snapshot(&lease_key);
        let current_tokens = meerkat_core::auth::rehydrate_marked_tokens_for_status_for_identity(
            credential_persistence.token_store().as_ref(),
            &auth_owner,
            selected.credential(),
            meerkat_core::auth::PersistedAuthMode::ApiKey,
            std::time::SystemTime::now().into(),
        )
        .await
        .expect("reopened machine verifies the retained current host credential owner")
        .expect("the actual token store retains its marked credential");
        assert_eq!(
            serde_json::to_value(&current_tokens).unwrap(),
            serde_json::to_value(&published_tokens).unwrap(),
        );
        assert_eq!(auth_owner.snapshot(&lease_key), before);
        assert!(before.credential_present);
        assert_eq!(
            before.phase,
            Some(meerkat_core::handles::AuthLeasePhase::Valid)
        );
        let fresh_claims = association(&runtime, controller, operation, selected.clone());
        let mut prompt = PromptInput::new(REOPEN_PROMPT, None);
        prompt.header.authority_association = Some(fresh_claims.clone());
        let input = Input::Prompt(prompt);
        let fresh_input_id = input.id().clone();
        assert_ne!(fresh_input_id, input_id);
        let current = NativeIngressContext::from_trusted_ingress(
            &input,
            principal("requester"),
            principal("ingress"),
            RealmId::parse("native-loop").unwrap(),
            super::super::evidence("stock-post-reopen-current-authentication"),
        )
        .unwrap()
        .with_controller_client(&input, pin.expect("fresh recovered actor pin"))
        .unwrap();
        let (accepted, completion) = machine
            .accept_input_with_completion(&session_id, input.with_ingress_context(current).unwrap())
            .await
            .expect("fresh actual admission after SQLite close/reopen");
        assert!(
            matches!(accepted, AcceptOutcome::Accepted { input_id: ref accepted_id, .. }
            if accepted_id == &fresh_input_id)
        );
        let completion = completion.expect("fresh native completion handle").wait();
        tokio::pin!(completion);
        // Keep this same completion future for the final result. A terminal
        // failure before HTTP4 is diagnostic failure, never a substitute for
        // the held fourth request and its sibling/effect assertions.
        tokio::time::timeout(Duration::from_secs(20), async {
            tokio::select! {
                biased;
                outcome = &mut completion => {
                    panic!("fresh work completed before the fourth HTTP request: {outcome:?}");
                }
                () = server.receiver.second_request.notified() => {}
            }
        })
        .await
        .expect("fresh siblings reach the fourth actual HTTP request");
        let bodies = server.receiver.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 4, "two real HTTP requests per fresh turn");
        if notification_probe {
            // Check the complete wire bodies, including top-level System
            // content, after the refused delivery and physical SQLite reopen.
            assert!(bodies.iter().all(|body| {
                let text = serde_json::to_string(body).unwrap();
                !text.contains(UNAUTHORIZED_NOTIFICATION)
                    && !text.contains(context_append::REFUSED_CONTEXT)
            }));
            assert!(
                bodies[2..].iter().all(|body| {
                    serde_json::to_string(body)
                        .unwrap()
                        .contains(context_append::PERMITTED_CONTEXT)
                }),
                "the permitted append survives reopen and reaches both fresh model requests"
            );
        }
        for body in &bodies {
            assert_eq!(body["model"], server.model());
            assert_eq!(body["stream"], true);
        }
        assert!(
            bodies[2]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "user"
                    && (message["content"].as_str() == Some(REOPEN_PROMPT)
                        || message["content"]
                            .as_array()
                            .is_some_and(|blocks| blocks
                                .iter()
                                .any(|block| block["type"] == "text"
                                    && block["text"] == REOPEN_PROMPT))))
        );
        assert_wire_sibling_feedback_with_ids(
            &bodies[3],
            REOPEN_DENIED_CALL,
            REOPEN_PERMITTED_CALL,
            4,
        );
        assert_eq!(*tools.0.lock().unwrap(), ["read_record", "read_record"]);
        let fresh_live = machine
            .input_state(&session_id, &fresh_input_id)
            .await
            .unwrap()
            .unwrap();
        let fresh_live_audit = stored_audit(&fresh_live);
        let fresh_run_id = fresh_live_audit
            .first()
            .expect("fresh protected live audit")
            .observation
            .run_id
            .clone()
            .unwrap();
        assert_ne!(
            fresh_run_id, run_id,
            "fresh input receives a new actual run"
        );
        let snapshot = machine
            .meerkat_machine_spine_snapshot(&session_id)
            .await
            .unwrap();
        assert_eq!(
            snapshot.control.current_run_id.as_ref(),
            Some(&fresh_run_id)
        );
        let before_fresh_finish = store
            .load_input_state(&runtime, &fresh_input_id)
            .await
            .unwrap()
            .unwrap();
        assert_original(&before_fresh_finish, &fresh_input_id, &fresh_claims);
        assert_eq!(
            before_fresh_finish.seed.last_run_id.as_ref(),
            Some(&fresh_run_id)
        );
        assert_eq!(
            before_fresh_finish
                .state
                .persisted_input
                .as_ref()
                .unwrap()
                .header()
                .authority_association
                .as_ref(),
            Some(&fresh_claims)
        );
        let frozen_fresh = serde_json::to_value(&before_fresh_finish).unwrap();
        assert!(fresh_live_audit.starts_with(&stored_audit(&before_fresh_finish)));
        server.receiver.finish.notify_one();
        let outcome = tokio::time::timeout(Duration::from_secs(20), completion.as_mut())
            .await
            .unwrap()
            .unwrap();
        let CompletionOutcome::Completed(result) = outcome else {
            panic!(
                "fresh denied operation must leave its permitted sibling and run available: {outcome:?}"
            )
        };
        assert_eq!(result.text, FINISHED);
        assert_eq!(result.session_id, session_id);
        assert!(result.terminal_cause_kind.is_none());
        let fresh_document = store
            .load_committed_whole_blob_snapshot(&runtime)
            .await
            .unwrap()
            .expect("fresh completion commits actual Session document");
        assert!(
            fresh_document.authority().store_revision()
                > recovered_document.authority().store_revision()
        );
        assert_ne!(fresh_document.bytes(), recovered_document.bytes());
        let decoded = Session::decode_whole_blob_document(fresh_document.bytes()).unwrap();
        assert_eq!(
            decoded.row_sha256_token(),
            fresh_document.authority().blob_sha256()
        );
        let fresh_saved = decoded.into_session();
        let batches = [
            (DENIED_CALL, PERMITTED_CALL),
            (REOPEN_DENIED_CALL, REOPEN_PERMITTED_CALL),
        ];
        assert_stock_document_batches(&fresh_saved, &session_id, &batches);
        let refusal = tool_feedback(fresh_saved.messages(), REOPEN_DENIED_CALL).unwrap();
        assert!(refusal.is_error);
        assert_eq!(
            serde_json::from_str::<Value>(&refusal.text_content()).unwrap(),
            ToolError::AuthorizationRefused { refusal: denied() }.to_error_payload()
        );
        let allowed = tool_feedback(fresh_saved.messages(), REOPEN_PERMITTED_CALL).unwrap();
        assert!(!allowed.is_error);
        assert_eq!(allowed.text_content(), "record-7 value");
        let authoritative = service
            .load_authoritative_session(&session_id)
            .await
            .unwrap()
            .unwrap();
        assert_stock_document_batches(&authoritative, &session_id, &batches);
        assert_eq!(
            serde_json::to_value(authoritative.messages()).unwrap(),
            serde_json::to_value(fresh_saved.messages()).unwrap()
        );
        let fresh_final_row = store
            .load_input_state(&runtime, &fresh_input_id)
            .await
            .unwrap()
            .unwrap();
        assert_original(&fresh_final_row, &fresh_input_id, &fresh_claims);
        assert_eq!(fresh_final_row.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            fresh_final_row.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        assert_eq!(
            fresh_final_row.seed.last_run_id.as_ref(),
            Some(&fresh_run_id)
        );
        assert_eq!(
            serde_json::to_value(&before_fresh_finish).unwrap(),
            frozen_fresh
        );
        let fresh_audit = stored_audit(&fresh_final_row);
        assert!(fresh_audit.starts_with(&fresh_live_audit));
        assert!(fresh_audit.len() > stored_audit(&before_fresh_finish).len());
        assert_stock_turn_audit(
            &fresh_audit,
            &session_id,
            &fresh_input_id,
            &fresh_run_id,
            REOPEN_DENIED_CALL,
            REOPEN_PERMITTED_CALL,
        );
        assert_eq!(store.load_input_states(&runtime).await.unwrap().len(), 2);
        let retained_original = store
            .load_input_state(&runtime, &input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&retained_original).unwrap(),
            expected_row
        );
        assert_eq!(stored_audit(&retained_original), audit);
        assert_eq!(server.receiver.bodies.lock().unwrap().len(), 4);
        assert_eq!(
            server.receiver.authorized_requests.load(Ordering::SeqCst),
            4
        );
        assert_eq!(*tools.0.lock().unwrap(), ["read_record", "read_record"]);
        fresh_terminal = Some((
            fresh_input_id,
            serde_json::to_value(&fresh_final_row).unwrap(),
            fresh_audit,
        ));
    }
    // Observe the no-replay effect oracle after the recovered worker drains.
    // A delayed replay cannot pass by racing the reconstruction assertions.
    machine
        .unregister_current_session_registration_until_terminal(&session_id)
        .await
        .expect("reconstructed worker reaches terminal teardown");
    service
        .discard_live_session(&session_id)
        .await
        .expect("reconstructed actor is discarded");
    drop(cleanup.lock().unwrap().take().unwrap());
    let retained_row = store
        .load_input_state(&runtime, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(&retained_row).unwrap(), expected_row);
    assert_eq!(stored_audit(&retained_row), audit);
    if let Some((fresh_input_id, expected_fresh, fresh_audit)) = fresh_terminal {
        let retained_fresh = store
            .load_input_state(&runtime, &fresh_input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(&retained_fresh).unwrap(),
            expected_fresh
        );
        assert_eq!(stored_audit(&retained_fresh), fresh_audit);
    }
    let completed_turns = if fresh_work_after_reopen { 2 } else { 1 };
    assert_eq!(
        server.receiver.bodies.lock().unwrap().len(),
        2 * completed_turns
    );
    assert_eq!(
        server.receiver.authorized_requests.load(Ordering::SeqCst),
        2 * completed_turns
    );
    assert_eq!(
        *tools.0.lock().unwrap(),
        vec!["read_record"; completed_turns]
    );
}

async fn run_stock_persistent_case(
    session_store: Arc<dyn meerkat::SessionStore>,
    store: Arc<dyn RuntimeStore>,
    reopen_database: Option<&std::path::Path>,
    fresh_work_after_reopen: bool,
) {
    assert!(
        !fresh_work_after_reopen || reopen_database.is_some(),
        "fresh post-reopen work requires the actual SQLite reopen path"
    );
    let server = if fresh_work_after_reopen {
        Server::start_with_tool_responses(vec![
            sibling_response(),
            sibling_response_with_ids(REOPEN_DENIED_CALL, REOPEN_PERMITTED_CALL),
        ])
        .await
    } else {
        Server::start().await
    };
    let client = http_client(&server);
    let credential_persistence = meerkat_core::auth::ProviderAuthPersistence::new(
        Arc::new(meerkat_auth_core::EphemeralTokenStore::new()),
        Arc::new(meerkat_auth_core::InMemoryCoordinator::new()),
    );
    run_stock_persistent_case_with_host(
        server,
        session_store,
        store,
        reopen_database,
        fresh_work_after_reopen,
        StockCredentialHost {
            client,
            credential_persistence,
        },
    )
    .await;
}

async fn run_stock_persistent_case_with_host(
    server: Server,
    session_store: Arc<dyn meerkat::SessionStore>,
    store: Arc<dyn RuntimeStore>,
    reopen_database: Option<&std::path::Path>,
    fresh_work_after_reopen: bool,
    host: StockCredentialHost,
) {
    run_stock_persistent_case_with_probe(
        server,
        session_store,
        store,
        reopen_database,
        fresh_work_after_reopen,
        host,
        None,
    )
    .await;
}

async fn run_stock_persistent_case_with_probe(
    server: Server,
    session_store: Arc<dyn meerkat::SessionStore>,
    store: Arc<dyn RuntimeStore>,
    reopen_database: Option<&std::path::Path>,
    fresh_work_after_reopen: bool,
    host: StockCredentialHost,
    controller_probe: Option<ControllerProbe>,
) {
    run_stock_persistent_case_with_options(
        server,
        session_store,
        store,
        reopen_database,
        StockPersistentOptions {
            fresh_work_after_reopen,
            notification_probe: false,
            controller_probe,
        },
        host,
    )
    .await;
}

async fn run_stock_persistent_case_with_options(
    mut server: Server,
    session_store: Arc<dyn meerkat::SessionStore>,
    store: Arc<dyn RuntimeStore>,
    reopen_database: Option<&std::path::Path>,
    options: StockPersistentOptions,
    host: StockCredentialHost,
) {
    let cleanup = CleanupSlot::new(None);
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        exercise_stock_persistent(
            &server,
            &cleanup,
            session_store,
            store,
            reopen_database,
            options,
            host,
        ),
    ))
    .catch_unwind()
    .await;
    server.receiver.finish.notify_one();
    let retained = cleanup.lock().unwrap().take();
    // The bound guards a hung teardown; the saga itself is joined until
    // terminal rather than answered `UnregisterInProgress` after the plain
    // unregister's caller grace.
    let cleanup_result = if let Some((machine, session)) = retained {
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
    server.reap().await;
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("stock persistent turn timed out: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    if let Some(result) = cleanup_result {
        result
            .expect("bounded native worker cleanup")
            .expect("actual persistent teardown");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_persistent_bundle_governed_turn_commits_session_input_and_audit() {
    run_stock_persistent_case(
        Arc::new(meerkat::MemoryStore::new()),
        Arc::new(InMemoryRuntimeStore::new()),
        None,
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_sqlite_whole_blob_governed_turn_commits_session_input_and_audit() {
    // The temporary database remains alive through actual native teardown.
    // Both co-tenant stores are their production SQLite backends.
    let directory = tempfile::tempdir().expect("actual SQLite fixture directory");
    let database = directory.path().join("stock-persistent.sqlite3");
    let session_store = Arc::new(
        meerkat::SqliteSessionStore::open(database.clone())
            .expect("actual SQLite session store opens"),
    );
    let runtime_store = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_whole_blob(database)
            .expect("actual SQLite WholeBlob runtime store opens"),
    );
    run_stock_persistent_case(session_store, runtime_store, None, false).await;
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_sqlite_whole_blob_reopen_preserves_governed_session_input_and_audit() {
    let directory = tempfile::tempdir().expect("actual SQLite reopen fixture directory");
    let database = directory.path().join("stock-reopened.sqlite3");
    let session_store = Arc::new(
        meerkat::SqliteSessionStore::open(database.clone())
            .expect("actual SQLite session store opens"),
    );
    let runtime_store = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_whole_blob(database.clone())
            .expect("actual SQLite WholeBlob runtime store opens"),
    );
    run_stock_persistent_case(session_store, runtime_store, Some(&database), false).await;
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_sqlite_whole_blob_reopen_runs_fresh_governed_model_tool_model_work() {
    let directory = tempfile::tempdir().expect("actual SQLite post-reopen work directory");
    let database = directory.path().join("stock-post-reopen-work.sqlite3");
    let session_store = Arc::new(
        meerkat::SqliteSessionStore::open(database.clone())
            .expect("actual SQLite session store opens"),
    );
    let runtime_store = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_whole_blob(database.clone())
            .expect("actual SQLite WholeBlob runtime store opens"),
    );
    run_stock_persistent_case(session_store, runtime_store, Some(&database), true).await;
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_sqlite_notification_without_authority_cannot_append_context_or_block_fresh_work() {
    // This is an idle source/audience-admission prerequisite, not active-turn
    // interruption or positively authorized Notification acceptance.
    let directory = tempfile::tempdir().expect("actual SQLite notification fixture directory");
    let database = directory.path().join("stock-notification.sqlite3");
    let session_store = Arc::new(meerkat::SqliteSessionStore::open(database.clone()).unwrap());
    let runtime_store =
        Arc::new(meerkat_runtime::SqliteRuntimeStore::new_whole_blob(database.clone()).unwrap());
    // This positive append survives after an existing conversation, so select
    // a real catalog profile that can represent nonleading System messages.
    // The shared E1 default remains unchanged for the other stock cases.
    assert!(
        meerkat_models::capabilities_for(Provider::Anthropic, NOTIFICATION_MODEL)
            .expect("cataloged Notification fixture model")
            .supports_mid_conversation_system_messages
    );
    let server = Server::start_with_model_and_tool_responses(
        NOTIFICATION_MODEL,
        vec![
            sibling_response_for_model(NOTIFICATION_MODEL, DENIED_CALL, PERMITTED_CALL),
            sibling_response_for_model(
                NOTIFICATION_MODEL,
                REOPEN_DENIED_CALL,
                REOPEN_PERMITTED_CALL,
            ),
        ],
    )
    .await;
    let host = StockCredentialHost {
        client: http_client(&server),
        credential_persistence: meerkat_core::auth::ProviderAuthPersistence::new(
            Arc::new(meerkat_auth_core::EphemeralTokenStore::new()),
            Arc::new(meerkat_auth_core::InMemoryCoordinator::new()),
        ),
    };
    run_stock_persistent_case_with_options(
        server,
        session_store,
        runtime_store,
        Some(&database),
        StockPersistentOptions {
            fresh_work_after_reopen: true,
            notification_probe: true,
            controller_probe: None,
        },
        host,
    )
    .await;
}

#[path = "stock_persistent/context_append.rs"]
mod context_append;
#[path = "stock_persistent/revalidation.rs"]
mod revalidation;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[path = "stock_persistent/process_reopen.rs"]
mod process_reopen;

// Independent selectors share the stock turn and its bounded cleanup. Neither
// relies on the other test or changes any production authorization owner.
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
async fn run_sqlite_controller_probe(probe: ControllerProbe) {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("stock-controller-admin.sqlite3");
    let server = Server::start().await;
    let client = http_client(&server);
    let credential_persistence = meerkat_auth_core::auth_store::TokenStoreBackend::File {
        root: directory.path().join("credentials"),
    }
    .open_with_refresh_authority()
    .unwrap();
    let host = StockCredentialHost {
        client,
        credential_persistence,
    };
    run_stock_persistent_case_with_probe(
        server,
        Arc::new(meerkat::SqliteSessionStore::open(database.clone()).unwrap()),
        Arc::new(meerkat_runtime::SqliteRuntimeStore::new_whole_blob(database.clone()).unwrap()),
        matches!(probe, ControllerProbe::TerminalRevocation).then_some(database.as_path()),
        false,
        host,
        Some(probe),
    )
    .await;
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_sqlite_terminal_controller_revocation_uses_actual_native_custody() {
    run_sqlite_controller_probe(ControllerProbe::TerminalRevocation).await;
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_sqlite_unfinished_controller_revocation_preserves_native_work() {
    run_sqlite_controller_probe(ControllerProbe::UnfinishedKeeper).await;
}
