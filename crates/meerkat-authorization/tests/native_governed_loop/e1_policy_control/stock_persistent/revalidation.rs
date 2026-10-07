//! Current native S2 revalidation through the stock persistent bundle.
//! The application table is the real configured invocation policy owner for this
//! fixture; native acceptance, actor, controller, grants and audit are unchanged.
//! This is not a JSONL/RPC authentication or governed wire projection claim.
use super::*;

#[derive(Clone, PartialEq, Eq)]
enum InvocationAction {
    Invoke,
}

#[derive(Clone, PartialEq, Eq)]
struct InvocationPermissionRow {
    requester: PrincipalRef,
    ingress_actor: PrincipalRef,
    realm: RealmId,
    runtime: LogicalRuntimeId,
    executor: PrincipalRef,
    action: InvocationAction,
}

async fn exercise_revalidated_stock_persistent(server: &Server, cleanup: &CleanupSlot) {
    let seed = Session::new();
    let session_id = seed.id().clone();
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let permission_row = InvocationPermissionRow {
        requester: principal("requester"),
        ingress_actor: principal("ingress"),
        realm: RealmId::parse("native-loop").unwrap(),
        runtime: runtime.clone(),
        executor: principal("executor"),
        action: InvocationAction::Invoke,
    };
    // The embedding application owns this exact invocation entitlement table.
    // No native accepted row, grant, controller or currentness token is copied.
    let permissions = Arc::new(Mutex::new(vec![permission_row.clone()]));
    let decisions: Arc<Mutex<Vec<(meerkat_core::InputId, Option<OperationRefusalKind>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let client = http_client(server);
    let selected = client
        .controller_model_selection()
        .expect("actual HTTP selection");
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("stock-revalidation-native-grants"),
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
    let ingress_controller = controller.clone();
    let ingress_operation = operation.clone();
    let ingress_selection = selected.clone();
    let ingress_permissions = permissions.clone();
    let ingress_decisions = decisions.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, input, current, claimed| {
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
            let refusal = denied();
            ingress_decisions
                .lock()
                .unwrap()
                .push((input.id().clone(), Some(refusal.kind())));
            return Err(refusal.into());
        }
        let allowed = ingress_permissions.lock().unwrap().iter().any(|row| {
            row.requester == *current.requester()
                && row.ingress_actor == *current.ingress_actor()
                && row.realm == *current.realm()
                && row.runtime == *runtime
                && row.executor == principal("executor")
                && row.action == InvocationAction::Invoke
        });
        if !allowed {
            let refusal = denied();
            ingress_decisions
                .lock()
                .unwrap()
                .push((input.id().clone(), Some(refusal.kind())));
            return Err(refusal.into());
        }
        ingress_decisions
            .lock()
            .unwrap()
            .push((input.id().clone(), None));
        Ok(())
    });
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let bundle = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        Arc::new(meerkat::MemoryStore::new()),
        store.clone(),
        Arc::new(meerkat::MemoryBlobStore::new()),
        NativeGrantWorkConfiguration {
            grants: grants.clone(),
            ingress,
            invocation_owner: Arc::new(InvocationOwner),
            operation_owner: Arc::new(HttpRecordOwner {
                selection: selected.clone(),
                endpoint: format!("{}/v1/messages", server.base_url),
            }),
        },
    )
    .expect("accepted candidate configures the bundle's actual persistent owner");
    assert_eq!(
        bundle.session_persistence_profile(),
        RuntimeSessionPersistenceProfile::WholeBlobV1
    );
    assert!(Arc::ptr_eq(&bundle.runtime_store(), &store));
    let configured_adapter = bundle.runtime_adapter();
    let tools = Arc::new(RecordingTools::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(client);
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
    *cleanup.lock().unwrap() = Some((machine.clone(), session_id.clone()));
    let reserved = service.reserve_create_session_admission().await.unwrap();
    let created = Box::pin(materialize_session_with_reserved_admission_and_actor_slot(
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

    let (pin, actor_witness) = {
        let actor_lease = service
            .acquire_live_session_actor_turn_boundary_lease(&session_id)
            .await
            .expect("current stock persistent actor under its turn boundary");
        let pin = service
            .pin_controller_client_for_actor(&actor_lease)
            .await
            .unwrap();
        (pin, actor_lease.witness().clone())
    };
    assert!(
        pin.selection() == &selected,
        "real actor keeps the actual HTTP client"
    );
    meerkat_auth_core::save_tokens_and_publish_lifecycle(
        meerkat_core::auth::ProviderAuthPersistence::new(
            Arc::new(meerkat_auth_core::EphemeralTokenStore::new()),
            Arc::new(meerkat_auth_core::InMemoryCoordinator::new()),
        ),
        machine.generated_auth_lease_handle(),
        selected.credential().clone(),
        meerkat_core::auth::PersistedTokens::api_key("synthetic-e1-loopback-only"),
    )
    .await
    .expect("same native generated credential owner");
    let credential_owner = machine.generated_auth_lease_handle();
    let credential_key = meerkat_core::handles::LeaseKey::from(selected.credential());
    let credential_before = credential_owner.snapshot(&credential_key);
    assert_eq!(
        credential_before.phase,
        Some(meerkat_core::handles::AuthLeasePhase::Valid)
    );
    assert!(credential_before.credential_present);
    let controller_before = grants
        .resolve_controller_lineage(
            std::slice::from_ref(&controller),
            &principal("executor"),
            None,
        )
        .expect("actual controller lineage before entitlement mutation");
    let operation_before = grants
        .resolve_lineage(
            std::slice::from_ref(&operation),
            &principal("executor"),
            None,
        )
        .expect("actual operation lineage before entitlement mutation");
    let claims = association(
        &runtime,
        controller.clone(),
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
        super::super::super::evidence("stock-revalidation-current-authentication"),
    )
    .unwrap()
    .with_controller_client(&input, pin.clone())
    .unwrap();
    let submitted = input.with_ingress_context(current).unwrap();
    assert!(
        decisions.lock().unwrap().is_empty(),
        "materialization did not admit work"
    );
    let removed_row = {
        let mut rows = permissions.lock().unwrap();
        assert_eq!(rows.len(), 1);
        rows.remove(0)
    };
    assert!(
        removed_row == permission_row,
        "only the actual requester invocation row changed"
    );
    let refused = machine
        .accept_input_with_completion(&session_id, submitted.clone())
        .await;
    // Apply this oracle only with the separately reviewed native admission
    // projection installed. The owner decision and all effect checks are unchanged.
    assert!(
        matches!(
            &refused,
            Err(meerkat_runtime::RuntimeDriverError::InputRefused { refusal })
                if refusal.kind() == OperationRefusalKind::Denied
        ),
        "projected native rejection class: {:?}",
        refused.as_ref().err()
    );
    let refusal_decisions = decisions.lock().unwrap().clone();
    assert!(
        !refusal_decisions.is_empty(),
        "actual installed ingress owner ran"
    );
    assert!(
        refusal_decisions
            .iter()
            .all(|(id, kind)| id == &input_id && *kind == Some(OperationRefusalKind::Denied)),
        "the actual owner returned typed denial; observed {} checks",
        refusal_decisions.len()
    );
    assert!(
        machine
            .input_state(&session_id, &input_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(store.load_input_states(&runtime).await.unwrap().is_empty());
    let rejected_snapshot = machine
        .meerkat_machine_spine_snapshot(&session_id)
        .await
        .unwrap();
    assert_eq!(
        rejected_snapshot.binding.driver_kind,
        MeerkatDriverKind::Persistent
    );
    assert_eq!(rejected_snapshot.binding.runtime_id, runtime);
    assert!(rejected_snapshot.control.current_run_id.is_none());
    assert!(
        server.receiver.bodies.lock().unwrap().is_empty(),
        "no model HTTP after denial"
    );
    assert_eq!(
        server.receiver.authorized_requests.load(Ordering::SeqCst),
        0
    );
    assert!(
        tools.0.lock().unwrap().is_empty(),
        "no physical tool entry after denial"
    );
    let rejected_document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .expect("initial committed document retained");
    assert_eq!(rejected_document.bytes(), initial_bytes.as_slice());
    assert_eq!(
        rejected_document.authority().store_revision(),
        initial_revision
    );
    assert_eq!(
        rejected_document.authority().blob_sha256(),
        initial_document.authority().blob_sha256()
    );
    let rejected_session = service
        .load_authoritative_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(rejected_session.messages()).unwrap(),
        serde_json::to_value(initial_document.session().messages()).unwrap()
    );
    assert_eq!(
        credential_owner.snapshot(&credential_key),
        credential_before
    );
    controller_before
        .check_current()
        .expect("controller owner was not mutated");
    operation_before
        .check_current()
        .expect("executor operation grant was not mutated");
    grants
        .resolve_controller_lineage(
            std::slice::from_ref(&controller),
            &principal("executor"),
            None,
        )
        .expect("controller remains usable");
    grants
        .resolve_lineage(
            std::slice::from_ref(&operation),
            &principal("executor"),
            None,
        )
        .expect("operation grant remains usable");
    {
        let actor_lease = service
            .acquire_live_session_actor_turn_boundary_lease_exact(&actor_witness)
            .await
            .unwrap()
            .expect("same live stock actor survived refusal");
        let still_pinned = service
            .pin_controller_client_for_actor(&actor_lease)
            .await
            .unwrap();
        assert!(still_pinned.selection() == pin.selection());
        assert!(
            Arc::ptr_eq(still_pinned.client(), pin.client()),
            "same actual controller client"
        );
    }
    // Restore the identical application row, not a grant or a fabricated pin.
    // Retrying the same process-bound input is safe because it was never accepted.
    permissions.lock().unwrap().push(removed_row);
    assert!(permissions.lock().unwrap().as_slice() == std::slice::from_ref(&permission_row));
    let (accepted, completion) = machine
        .accept_input_with_completion(&session_id, submitted)
        .await
        .expect("restored entitlement admits through the same native owner");
    assert!(
        decisions
            .lock()
            .unwrap()
            .iter()
            .any(|(id, kind)| id == &input_id && kind.is_none()),
        "current owner re-read the restored row"
    );
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
        assert_eq!(body["model"], E1_MODEL);
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

    assert_eq!(
        store.load_input_states(&runtime).await.unwrap().len(),
        1,
        "only the restored invocation became a durable input"
    );
    assert!(decisions.lock().unwrap().starts_with(&refusal_decisions));
    assert_eq!(
        credential_owner.snapshot(&credential_key),
        credential_before
    );
    controller_before
        .check_current()
        .expect("same controller grant after completion");
    operation_before
        .check_current()
        .expect("same executor grant after completion");
    {
        let actor_lease = service
            .acquire_live_session_actor_turn_boundary_lease_exact(&actor_witness)
            .await
            .unwrap()
            .expect("same actor completed the restored invocation");
        let still_pinned = service
            .pin_controller_client_for_actor(&actor_lease)
            .await
            .unwrap();
        assert!(still_pinned.selection() == pin.selection());
        assert!(Arc::ptr_eq(still_pinned.client(), pin.client()));
    }

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
    for record in &audit {
        assert_eq!(record.observation.run_id.as_ref(), Some(&run_id));
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(record.contributors[0].input_id, input_id);
        assert!(record.contributors[0].requester == principal("requester"));
        assert!(record.contributors[0].logical_executor == principal("executor"));
        assert!(record.contributors[0].represented_subject.is_none());
        assert!(
            matches!(&record.observation.execution_scope, OperationExecutionScope::RuntimeInput {
            owner_session_id, submitted_input_id, canonical_input_id, ..
        } if owner_session_id == &session_id && submitted_input_id == &input_id && canonical_input_id == &input_id)
        );
    }
    let refused: Vec<_> = audit
        .iter()
        .filter(|record| {
            matches!(&record.observation.observation,
        AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
        if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
            if call_id == DENIED_CALL && tool_name == "delete_record"))
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
            if call_id == PERMITTED_CALL && tool_name == "read_record"))
        })
        .collect();
    assert_eq!(prepared.len(), 1);
    let read_id = &prepared[0].observation.operation_id;
    assert!(
        audit
            .iter()
            .any(|record| &record.observation.operation_id == read_id
                && matches!(record.observation.observation, AuditObservation::Entry))
    );
    assert!(
        audit
            .iter()
            .any(|record| &record.observation.operation_id == read_id
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
        assert!(
            audit
                .iter()
                .any(|record| &record.observation.operation_id == operation_id
                    && matches!(record.observation.observation, AuditObservation::Entry))
        );
        assert!(
            audit
                .iter()
                .any(|record| &record.observation.operation_id == operation_id
                    && matches!(
                        record.observation.observation,
                        AuditObservation::Outcome {
                            outcome: OperationObservedOutcome::HttpResponse { status: 200 }
                        }
                    ))
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_persistent_revalidates_requester_before_admission_and_preserves_actor() {
    let mut server = Server::start().await;
    let cleanup = CleanupSlot::new(None);
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        exercise_revalidated_stock_persistent(&server, &cleanup),
    ))
    .catch_unwind()
    .await;
    server.receiver.finish.notify_one();
    let retained = cleanup.lock().unwrap().take();
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
