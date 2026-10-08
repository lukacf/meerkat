//! Cold-process qualification of the existing stock SQLite recovery path.
//! Each host explicitly issues current grants and authenticates fresh input.
//! Historical associations and transcripts are audit data, never authority.
use super::*;

const SELECTOR: &str = "e1_policy_control::stock_persistent::process_reopen::stock_sqlite_separate_process_reopen_runs_fresh_governed_model_tool_model_work";
const PHASE_ENV: &str = "MEERKAT_STOCK_REOPEN_TEST_PHASE";
const ROOT_ENV: &str = "MEERKAT_STOCK_REOPEN_TEST_ROOT";
const COLD_DENIED_CALL: &str = "cold-process-delete";
const COLD_PERMITTED_CALL: &str = "cold-process-read";
const COLD_PROMPT: &str = "After process restart, attempt two fresh record actions";

fn cold_binding() -> AuthBindingRef {
    // Stable trusted host configuration, not a binding adopted from an input.
    AuthBindingRef {
        realm: RealmId::parse("native-loop").unwrap(),
        binding: BindingId::parse("stock-cold-loopback").unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

fn file_credentials(root: &std::path::Path) -> meerkat_core::auth::ProviderAuthPersistence {
    meerkat_auth_core::auth_store::TokenStoreBackend::File {
        root: root.join("credentials"),
    }
    .open_with_refresh_authority()
    .expect("file credentials keep their canonical cross-process refresh authority")
}

fn sqlite_stores(
    root: &std::path::Path,
) -> (Arc<dyn meerkat::SessionStore>, Arc<dyn RuntimeStore>) {
    let database = root.join("stock-cold.sqlite3");
    (
        Arc::new(meerkat::SqliteSessionStore::open(database.clone()).unwrap()),
        Arc::new(meerkat_runtime::SqliteRuntimeStore::new_whole_blob(database).unwrap()),
    )
}

async fn only_session(store: &Arc<dyn RuntimeStore>) -> SessionId {
    let sessions = store
        .list_runtime_session_catalog_entries(meerkat_core::SessionFilter::default())
        .await
        .unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "discover the real session without a receipt file"
    );
    sessions[0].session_id().clone()
}

async fn write_completed_turn(root: &std::path::Path) {
    let server = Server::start().await;
    let host = StockCredentialHost {
        client: http_client_with_binding(&server, cold_binding()),
        credential_persistence: file_credentials(root),
    };
    let (session_store, store) = sqlite_stores(root);
    run_stock_persistent_case_with_host(server, session_store, store, None, false, host).await;
}

async fn read_and_run_fresh_turn(root: &std::path::Path, cleanup: &CleanupSlot, server: &Server) {
    let (session_store, store) = sqlite_stores(root);
    let session_id = only_session(&store).await;
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let original_rows = store.load_input_states_strict(&runtime).await.unwrap();
    assert_eq!(original_rows.len(), 1);
    let original = &original_rows[0];
    assert_eq!(original.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(
        original.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed)
    );
    let original_input_id = original.state.input_id.clone();
    let original_run_id = original.seed.last_run_id.clone().unwrap();
    let frozen_original = serde_json::to_value(original).unwrap();
    let original_claims = original.state.authority_contributors[0]
        .association()
        .clone();
    assert_stock_turn_audit(
        &stored_audit(original),
        &session_id,
        &original_input_id,
        &original_run_id,
        DENIED_CALL,
        PERMITTED_CALL,
    );
    let original_document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    assert_stock_document(original_document.session(), &session_id);
    let original_message_count = original_document.session().messages().len();
    let frozen_original_messages =
        serde_json::to_value(original_document.session().messages()).unwrap();
    let frozen_bytes = original_document.bytes().to_vec();
    let frozen_authority = original_document.authority().clone();

    let client = http_client_with_binding(server, cold_binding());
    let selected = client.controller_model_selection().unwrap();
    // The reader host issues new lineages. It does not restore a grant owner
    // or adopt old association fields as current permission.
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("stock-cold-current-host-grants"),
                generation: 2,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .unwrap(),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("cold-controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .unwrap();
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("cold-read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .unwrap();
    let ingress_controller = controller.clone();
    let ingress_operation = operation.clone();
    let ingress_selection = selected.clone();
    let ingress_original = original_claims.clone();
    let original_ingress_accepted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed_original_ingress = original_ingress_accepted.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("native-loop").unwrap()
        {
            return Err(denied().into());
        }
        if claimed == &ingress_original {
            // Authentication accepts this exact historical claim so only the
            // current grant owner can refuse its missing controller lineage.
            observed_original_ingress.store(true, Ordering::SeqCst);
            return Ok(());
        }
        if claimed
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
    let bundle = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        session_store,
        store.clone(),
        Arc::new(meerkat::MemoryBlobStore::new()),
        NativeGrantWorkConfiguration {
            grants,
            ingress,
            invocation_owner: Arc::new(InvocationOwner),
            operation_owner: Arc::new(HttpRecordOwner {
                selection: selected.clone(),
                endpoint: format!("{}/v1/messages", server.base_url),
            }),
        },
    )
    .expect("cold host explicitly installs its current native owners");
    let configured_adapter = bundle.runtime_adapter();
    let tools = Arc::new(RecordingTools::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(client);
    builder.default_tool_dispatcher = Some(tools.clone());
    let (service, machine) = build_runtime_backed_service_with_default_reconfigure_host(
        builder,
        2,
        bundle,
        root.join("unused-cold-host.toml"),
    );
    assert!(Arc::ptr_eq(&machine, &configured_adapter));
    assert!(Arc::ptr_eq(&service.runtime_store(), &store));
    *cleanup.lock().unwrap() = Some((machine.clone(), session_id.clone()));
    let persisted = service
        .load_authoritative_session(&session_id)
        .await
        .unwrap()
        .unwrap();
    assert_stock_document(&persisted, &session_id);
    let recovered = build_recovered_session(
        persisted.clone(),
        &SurfaceSessionRecoveryOverrides::default(),
        SurfaceSessionRecoveryContext::default(),
    )
    .unwrap();
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
    .expect("cold stock actor reconstructs from the committed session");
    assert_eq!(created.session_id, session_id);
    let pin = {
        let actor_lease = service
            .acquire_live_session_actor_turn_boundary_lease(&session_id)
            .await
            .unwrap();
        service
            .pin_controller_client_for_actor(&actor_lease)
            .await
            .unwrap()
    };
    assert!(pin.selection() == &selected);
    assert!(server.receiver.bodies.lock().unwrap().is_empty());
    assert!(tools.0.lock().unwrap().is_empty());
    let recovered_original = machine
        .input_state(&session_id, &original_input_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&recovered_original).unwrap(),
        frozen_original
    );
    let reconstructed_document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    // Actor publication may advance the document revision. It cannot replay
    // old work or rewrite the committed transcript and protected input audit.
    assert_eq!(reconstructed_document.authority().session_id(), &session_id);
    assert!(
        reconstructed_document.authority().store_revision() >= frozen_authority.store_revision()
    );
    assert_eq!(
        serde_json::to_value(reconstructed_document.session().messages()).unwrap(),
        frozen_original_messages,
    );
    let reconstructed =
        Session::decode_whole_blob_document(reconstructed_document.bytes()).unwrap();
    assert_eq!(
        reconstructed.row_sha256_token(),
        reconstructed_document.authority().blob_sha256(),
    );

    let persistence = file_credentials(root);
    let token_store = persistence.token_store();
    let token_key = meerkat_core::auth::TokenKey::from_credential_identity(selected.credential());
    let stored_tokens = token_store
        .load(&token_key)
        .await
        .unwrap()
        .expect("writer's marked file credential survives process exit");
    assert_eq!(
        stored_tokens.primary_secret.as_deref(),
        Some("synthetic-e1-loopback-only")
    );
    let auth_owner = machine.generated_auth_lease_handle();
    let lease_key =
        meerkat_core::handles::LeaseKey::from_credential_identity(selected.credential());
    let cold = auth_owner.snapshot(&lease_key);
    assert!(
        !cold.credential_present,
        "writer process auth registry cannot survive here"
    );
    assert_eq!(cold.generation, 0);

    let claims = association(&runtime, controller, operation, selected.clone());
    assert_ne!(
        &claims,
        original.state.authority_contributors[0].association()
    );
    let mut prompt = PromptInput::new(COLD_PROMPT, None);
    prompt.header.authority_association = Some(claims.clone());
    let input = Input::Prompt(prompt);
    let input_id = input.id().clone();
    assert_ne!(input_id, original_input_id);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::super::super::evidence("stock-cold-current-authentication"),
    )
    .unwrap()
    .with_controller_client(&input, pin.clone())
    .unwrap();
    let input = input.with_ingress_context(current).unwrap();

    // Current grants and durable token bytes do not replace the cold native
    // credential owner. Keep this exact input for retry after its restoration.
    let unavailable = machine
        .accept_input_with_completion(&session_id, input.clone())
        .await;
    assert!(
        matches!(
            &unavailable,
            Err(
                meerkat_runtime::traits::RuntimeDriverError::ControllerReadinessUnavailable {
                    reason:
                        meerkat_runtime::traits::ControllerReadinessFailure::CredentialUnusable {
                            disposition:
                                meerkat_core::handles::CredentialUseDisposition::LeaseAbsent,
                        },
                }
            )
        ),
        "a cold credential owner must return typed readiness, not permission denial: {unavailable:?}",
    );
    assert!(server.receiver.bodies.lock().unwrap().is_empty());
    assert_eq!(
        server.receiver.authorized_requests.load(Ordering::SeqCst),
        0
    );
    assert!(tools.0.lock().unwrap().is_empty());
    assert!(
        machine
            .input_state(&session_id, &input_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .load_input_state(&runtime, &input_id)
            .await
            .unwrap()
            .is_none()
    );
    let rows_before_restore = store.load_input_states_strict(&runtime).await.unwrap();
    assert_eq!(rows_before_restore.len(), 1);
    assert_eq!(
        serde_json::to_value(&rows_before_restore[0]).unwrap(),
        frozen_original,
        "cold admission cannot change the original durable input or protected audit",
    );
    assert_eq!(
        serde_json::to_value(
            machine
                .input_state(&session_id, &original_input_id)
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        frozen_original,
        "cold admission cannot change the original live input or protected audit",
    );
    let document_before_restore = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        document_before_restore.bytes(),
        reconstructed_document.bytes(),
        "cold admission cannot rewrite the committed session or transcript",
    );
    assert_eq!(
        document_before_restore.authority(),
        reconstructed_document.authority(),
    );
    assert_eq!(
        token_store.load(&token_key).await.unwrap(),
        Some(stored_tokens.clone()),
    );

    let publication = meerkat_core::auth::tokens_lifecycle_publication(&stored_tokens)
        .expect("writer persisted an actual credential lifecycle publication");
    let restored = meerkat_core::auth::rehydrate_marked_tokens_for_status_for_identity(
        token_store.as_ref(),
        &auth_owner,
        selected.credential(),
        meerkat_core::auth::PersistedAuthMode::ApiKey,
        std::time::SystemTime::now().into(),
    )
    .await
    .expect("existing generated owner restores the marked durable credential")
    .expect("cold restore requires a valid durable publication marker");
    assert_eq!(restored, stored_tokens);
    let restored_snapshot = auth_owner.snapshot(&lease_key);
    assert!(restored_snapshot.credential_present);
    assert_eq!(
        restored_snapshot.phase,
        Some(meerkat_core::handles::AuthLeasePhase::Valid)
    );
    assert_eq!(restored_snapshot.phase, publication.phase);
    assert_eq!(
        restored_snapshot.generation,
        publication.generation.unwrap()
    );
    assert_eq!(
        restored_snapshot.credential_published_at_millis,
        publication.credential_published_at_millis
    );
    assert!(restored_snapshot.generation > cold.generation);
    assert_eq!(
        token_store.load(&token_key).await.unwrap(),
        Some(stored_tokens)
    );

    assert_eq!(
        original_claims.candidate().controller_model.as_ref(),
        Some(pin.selection()),
        "the stale-grant attempt uses the actual fresh pinned client",
    );
    let mut stale_prompt = PromptInput::new("Attempt work with the writer's old grants", None);
    stale_prompt.header.authority_association = Some(original_claims.clone());
    let stale_input = Input::Prompt(stale_prompt);
    let stale_input_id = stale_input.id().clone();
    assert_ne!(stale_input_id, original_input_id);
    let stale_current = NativeIngressContext::from_trusted_ingress(
        &stale_input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        super::super::super::evidence("stock-cold-old-claims-current-authentication"),
    )
    .unwrap()
    .with_controller_client(&stale_input, pin.clone())
    .unwrap();
    let refused = machine
        .accept_input_with_completion(
            &session_id,
            stale_input.with_ingress_context(stale_current).unwrap(),
        )
        .await;
    assert!(
        matches!(&refused, Err(meerkat_runtime::traits::RuntimeDriverError::InputRefused { refusal })
            if refusal.kind() == OperationRefusalKind::Denied),
        "current grant authority must refuse historical permission: {refused:?}",
    );
    assert!(
        original_ingress_accepted.load(Ordering::SeqCst),
        "fresh authentication must accept the old claims before current grant denial",
    );
    assert!(server.receiver.bodies.lock().unwrap().is_empty());
    assert_eq!(
        server.receiver.authorized_requests.load(Ordering::SeqCst),
        0
    );
    assert!(tools.0.lock().unwrap().is_empty());
    assert!(
        machine
            .input_state(&session_id, &stale_input_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .load_input_state(&runtime, &stale_input_id)
            .await
            .unwrap()
            .is_none()
    );
    let rows_after_refusal = store.load_input_states_strict(&runtime).await.unwrap();
    assert_eq!(
        rows_after_refusal.len(),
        1,
        "pre-admission denial cannot add a durable row"
    );
    assert_eq!(
        serde_json::to_value(&rows_after_refusal[0]).unwrap(),
        frozen_original
    );
    let document_after_refusal = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    let decoded_after_refusal =
        Session::decode_whole_blob_document(document_after_refusal.bytes()).unwrap();
    assert_eq!(
        decoded_after_refusal.row_sha256_token(),
        document_after_refusal.authority().blob_sha256(),
    );
    assert_eq!(
        serde_json::to_value(document_after_refusal.session().messages()).unwrap(),
        frozen_original_messages,
        "a refused old-grant input cannot enter the committed transcript",
    );

    assert_ne!(input_id, stale_input_id);
    let (accepted, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("the same cold input is admissible after canonical credential restoration");
    assert!(
        matches!(accepted, AcceptOutcome::Accepted { input_id: ref accepted_id, .. }
        if accepted_id == &input_id)
    );
    let completion = completion.unwrap();
    tokio::time::timeout(
        Duration::from_secs(20),
        server.receiver.second_request.notified(),
    )
    .await
    .unwrap();
    let bodies = server.receiver.bodies.lock().unwrap().clone();
    assert_eq!(
        bodies.len(),
        2,
        "old HTTP work must not replay in the reader process"
    );
    for body in &bodies {
        assert_eq!(body["model"], E1_MODEL);
        assert_eq!(body["stream"], true);
    }
    assert!(
        bodies[0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "user"
                && (message["content"].as_str() == Some(COLD_PROMPT)
                    || message["content"].as_array().is_some_and(|blocks| blocks
                        .iter()
                        .any(|block| block["type"] == "text" && block["text"] == COLD_PROMPT))))
    );
    assert_wire_sibling_feedback_with_ids(&bodies[1], COLD_DENIED_CALL, COLD_PERMITTED_CALL, 4);
    assert_eq!(*tools.0.lock().unwrap(), ["read_record"]);
    let live = machine
        .input_state(&session_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    let live_audit = stored_audit(&live);
    let run_id = live_audit
        .first()
        .unwrap()
        .observation
        .run_id
        .clone()
        .unwrap();
    assert_ne!(run_id, original_run_id);
    let before_finish = store
        .load_input_state(&runtime, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert_original(&before_finish, &input_id, &claims);
    assert_eq!(before_finish.seed.last_run_id.as_ref(), Some(&run_id));
    let frozen_before = serde_json::to_value(&before_finish).unwrap();
    server.receiver.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("cold refusal must leave sibling work and the run available: {outcome:?}")
    };
    assert_eq!(result.text, FINISHED);
    assert_eq!(result.session_id, session_id);
    assert!(result.terminal_cause_kind.is_none());
    let final_document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    assert!(
        final_document.authority().store_revision()
            > reconstructed_document.authority().store_revision()
    );
    assert_ne!(final_document.bytes(), frozen_bytes.as_slice());
    let decoded = Session::decode_whole_blob_document(final_document.bytes()).unwrap();
    assert_eq!(
        decoded.row_sha256_token(),
        final_document.authority().blob_sha256()
    );
    let saved = decoded.into_session();
    assert!(saved.messages().len() >= original_message_count);
    assert_eq!(
        serde_json::to_value(&saved.messages()[..original_message_count]).unwrap(),
        frozen_original_messages,
        "fresh work must preserve the exact original transcript prefix",
    );
    assert_stock_document_batches(
        &saved,
        &session_id,
        &[
            (DENIED_CALL, PERMITTED_CALL),
            (COLD_DENIED_CALL, COLD_PERMITTED_CALL),
        ],
    );
    let refused = tool_feedback(saved.messages(), COLD_DENIED_CALL).unwrap();
    assert!(refused.is_error);
    assert_eq!(
        serde_json::from_str::<Value>(&refused.text_content()).unwrap(),
        ToolError::AuthorizationRefused { refusal: denied() }.to_error_payload()
    );
    let allowed = tool_feedback(saved.messages(), COLD_PERMITTED_CALL).unwrap();
    assert!(!allowed.is_error);
    assert_eq!(allowed.text_content(), "record-7 value");
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
    assert_eq!(serde_json::to_value(&before_finish).unwrap(), frozen_before);
    let final_audit = stored_audit(&final_row);
    assert!(final_audit.starts_with(&live_audit));
    assert_stock_turn_audit(
        &final_audit,
        &session_id,
        &input_id,
        &run_id,
        COLD_DENIED_CALL,
        COLD_PERMITTED_CALL,
    );
    let frozen_final = serde_json::to_value(&final_row).unwrap();
    machine
        .unregister_current_session_registration_until_terminal(&session_id)
        .await
        .unwrap();
    service.discard_live_session(&session_id).await.unwrap();
    drop(cleanup.lock().unwrap().take().unwrap());
    assert_eq!(
        store
            .load_input_states_strict(&runtime)
            .await
            .unwrap()
            .len(),
        2
    );
    let retained_original = store
        .load_input_state(&runtime, &original_input_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(retained_original).unwrap(),
        frozen_original
    );
    let retained_fresh = store
        .load_input_state(&runtime, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::to_value(retained_fresh).unwrap(), frozen_final);
    assert_eq!(server.receiver.bodies.lock().unwrap().len(), 2);
    assert_eq!(
        server.receiver.authorized_requests.load(Ordering::SeqCst),
        2
    );
    assert_eq!(*tools.0.lock().unwrap(), ["read_record"]);
}

async fn run_reader(root: &std::path::Path) {
    let mut server = Server::start_with_tool_response(sibling_response_with_ids(
        COLD_DENIED_CALL,
        COLD_PERMITTED_CALL,
    ))
    .await;
    let cleanup = CleanupSlot::new(None);
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(60),
        read_and_run_fresh_turn(root, &cleanup, &server),
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
        Ok(Err(error)) => panic!("cold stock persistent turn timed out: {error}"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
    if let Some(result) = cleanup_result {
        result.unwrap().unwrap();
    }
}

async fn run_child(root: &std::path::Path, phase: &str) {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", SELECTOR, "--nocapture", "--test-threads=1"])
        .env(PHASE_ENV, phase)
        .env(ROOT_ENV, root)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(90), command.output())
        .await
        .expect("bounded existing integration test child")
        .unwrap();
    assert!(output.status.success(), "{phase} child failed: {output:?}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed; 0 failed;"),
        "the exact child selector must execute, not empty-pass: {output:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_sqlite_separate_process_reopen_runs_fresh_governed_model_tool_model_work() {
    if let Some(phase) = std::env::var_os(PHASE_ENV) {
        let root = PathBuf::from(std::env::var_os(ROOT_ENV).expect("child fixture root"));
        match phase.to_str().expect("UTF-8 fixture phase") {
            "write" => write_completed_turn(&root).await,
            "read" => run_reader(&root).await,
            other => panic!("unknown cold-process fixture phase: {other}"),
        }
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    run_child(directory.path(), "write").await;
    let (session_store, store) = sqlite_stores(directory.path());
    let session_id = only_session(&store).await;
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let rows = store.load_input_states_strict(&runtime).await.unwrap();
    assert_eq!(rows.len(), 1);
    let original_input_id = rows[0].state.input_id.clone();
    let frozen_original = serde_json::to_value(&rows[0]).unwrap();
    let original_document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    assert_stock_document(original_document.session(), &session_id);
    let original_message_count = original_document.session().messages().len();
    let frozen_original_messages =
        serde_json::to_value(original_document.session().messages()).unwrap();
    // Parent inspection owns no governed worker or credential registry.
    drop(session_store);
    drop(store);
    run_child(directory.path(), "read").await;
    let (_session_store, store) = sqlite_stores(directory.path());
    assert_eq!(only_session(&store).await, session_id);
    let rows = store.load_input_states_strict(&runtime).await.unwrap();
    assert_eq!(rows.len(), 2, "only two newly submitted inputs are durable");
    let original = rows
        .iter()
        .find(|row| row.state.input_id == original_input_id)
        .unwrap();
    assert_eq!(serde_json::to_value(original).unwrap(), frozen_original);
    let fresh = rows
        .iter()
        .find(|row| row.state.input_id != original_input_id)
        .unwrap();
    assert_ne!(fresh.seed.last_run_id, original.seed.last_run_id);
    assert_eq!(fresh.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(
        fresh.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed)
    );
    assert_stock_turn_audit(
        &stored_audit(fresh),
        &session_id,
        &fresh.state.input_id,
        fresh.seed.last_run_id.as_ref().unwrap(),
        COLD_DENIED_CALL,
        COLD_PERMITTED_CALL,
    );
    let document = store
        .load_committed_whole_blob_snapshot(&runtime)
        .await
        .unwrap()
        .unwrap();
    assert!(document.session().messages().len() >= original_message_count);
    assert_eq!(
        serde_json::to_value(&document.session().messages()[..original_message_count]).unwrap(),
        frozen_original_messages,
        "reader process must preserve the exact original transcript prefix",
    );
    assert_stock_document_batches(
        document.session(),
        &session_id,
        &[
            (DENIED_CALL, PERMITTED_CALL),
            (COLD_DENIED_CALL, COLD_PERMITTED_CALL),
        ],
    );
}
