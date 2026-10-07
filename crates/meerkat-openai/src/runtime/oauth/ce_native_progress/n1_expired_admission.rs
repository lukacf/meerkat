//! N1: a real request-time refresh owner must be reachable before governed admission.
//! OpenAI Responses uses a supported external authorizer with a real Azure AD
//! token exchange. This is not managed ChatGPT StaticLease refresh coverage.
use super::*;
use axum::extract::Form;
use meerkat_auth_core::authorizers::{AzureAdAuthorizer, AzureClientCredentials};
use meerkat_core::HttpAuthorizationRequest;
use meerkat_runtime::traits::{ControllerReadinessFailure, RuntimeDriverError};

#[derive(Clone, Copy, Debug)]
enum CredentialCase {
    Current,
    Expired,
    Released,
    DeniedBeforeRefresh,
    CancelDuringRefresh,
    PolicyChangedDuringRefresh,
}

impl CredentialCase {
    fn expired(self) -> bool {
        !matches!(self, Self::Current | Self::Released)
    }

    fn gated(self) -> bool {
        matches!(
            self,
            Self::CancelDuringRefresh | Self::PolicyChangedDuringRefresh
        )
    }
}

struct CloudEndpoint {
    common: Arc<Endpoint>,
    authority: GeneratedAuthLeaseHandle,
    lease: LeaseKey,
    first_expiry_secs: u64,
    gate_refresh: bool,
    token_forms: Mutex<Vec<HashMap<String, String>>>,
    model_bearers: Mutex<Vec<String>>,
    model_authority: Mutex<Vec<CredentialUseDisposition>>,
    native: Mutex<Option<(std::sync::Weak<MeerkatMachine>, SessionId)>>,
    ingress_inputs: Mutex<Vec<meerkat_core::InputId>>,
    refresh_before_admission: Mutex<Vec<bool>>,
    refresh_phases: Mutex<Vec<Option<AuthLeasePhase>>>,
}

// Failure paths must release physical receiver waits and cancel only the
// caller task. The runtime-owned maintenance remains responsible for closure.
struct RefreshCaseCleanup {
    endpoint: Arc<Endpoint>,
    caller: Option<tokio::task::AbortHandle>,
}

impl Drop for RefreshCaseCleanup {
    fn drop(&mut self) {
        self.endpoint.refresh_release.add_permits(1);
        self.endpoint.model_finish.add_permits(1);
        if let Some(caller) = &self.caller {
            caller.abort();
        }
    }
}

async fn cloud_token(
    State(endpoint): State<Arc<CloudEndpoint>>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    let call = {
        let mut forms = endpoint.token_forms.lock().unwrap();
        forms.push(form);
        forms.len()
    };
    if call > 1 {
        let (machine, session) = endpoint.native.lock().unwrap().clone().unwrap();
        let machine = machine
            .upgrade()
            .expect("fixture native owner remains alive");
        let inputs = endpoint.ingress_inputs.lock().unwrap().clone();
        let mut before_admission = true;
        for input in &inputs {
            before_admission &= machine
                .input_state(&session, input)
                .await
                .unwrap()
                .is_none();
        }
        endpoint
            .refresh_before_admission
            .lock()
            .unwrap()
            .push(before_admission);
        endpoint
            .refresh_phases
            .lock()
            .unwrap()
            .push(endpoint.authority.snapshot(&endpoint.lease).phase);
        if !before_admission {
            // Letting Expired through and refreshing inside the model call is
            // not readiness repair: the actual native row already exists.
            return StatusCode::CONFLICT.into_response();
        }
    }
    if call > 1 && endpoint.gate_refresh {
        endpoint.common.refresh_arrived.notify_one();
        match endpoint.common.refresh_release.acquire().await {
            Ok(permit) => permit.forget(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }
    Json(json!({
        "access_token": format!("n1-access-{call}"),
        "expires_in": if call == 1 { endpoint.first_expiry_secs } else { 3600 },
    }))
    .into_response()
}

async fn cloud_model(
    State(endpoint): State<Arc<CloudEndpoint>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    let bearer = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let token_count = endpoint.token_forms.lock().unwrap().len();
    endpoint.model_bearers.lock().unwrap().push(bearer.clone());
    let disposition = endpoint
        .authority
        .resolve_credential_use_admission(&endpoint.lease, CredentialUseIntent::HoldAuthority)
        .unwrap();
    endpoint.model_authority.lock().unwrap().push(disposition);
    // The physical receiver requires the actual exchanged current token and
    // current generated authority, not merely a header supplied by a fixture.
    if bearer != format!("Bearer n1-access-{token_count}")
        || disposition != CredentialUseDisposition::Authorized
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let has_result = body["input"].as_array().is_some_and(|input| {
        input
            .iter()
            .any(|item| item["type"] == "function_call_output")
    });
    endpoint.common.model_requests.lock().unwrap().push(body);
    let output = if has_result {
        endpoint.common.model_result_arrived.notify_one();
        match endpoint.common.model_finish.acquire().await {
            Ok(permit) => permit.forget(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
        json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":FINISHED}]}])
    } else {
        json!([{"type":"function_call","call_id":CALL,"name":"read_record","arguments":"{\"record\":\"record-7\"}"}])
    };
    let event = json!({"type":"response.completed", "response":{"status":"completed", "output":output, "usage":{"input_tokens":1,"output_tokens":1}}});
    (
        StatusCode::OK,
        [("content-type", "text/event-stream")],
        format!("data: {event}\n\ndata: [DONE]\n\n"),
    )
        .into_response()
}

async fn cloud_server(
    authority: GeneratedAuthLeaseHandle,
    lease: LeaseKey,
    case: CredentialCase,
) -> (Server, Arc<CloudEndpoint>) {
    let common = Arc::new(Endpoint {
        model_requests: Mutex::new(Vec::new()),
        model_result_arrived: Notify::new(),
        model_finish: Semaphore::new(0),
        refresh_arrived: Notify::new(),
        refresh_release: Semaphore::new(0),
        refresh_requests: Mutex::new(Vec::new()),
    });
    let endpoint = Arc::new(CloudEndpoint {
        common: common.clone(),
        authority,
        lease,
        first_expiry_secs: if case.expired() { 0 } else { 3600 },
        gate_refresh: case.gated(),
        token_forms: Mutex::new(Vec::new()),
        model_bearers: Mutex::new(Vec::new()),
        model_authority: Mutex::new(Vec::new()),
        native: Mutex::new(None),
        ingress_inputs: Mutex::new(Vec::new()),
        refresh_before_admission: Mutex::new(Vec::new()),
        refresh_phases: Mutex::new(Vec::new()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/v1/responses", post(cloud_model))
        .route("/token", post(cloud_token))
        .with_state(endpoint.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (
        Server {
            base,
            endpoint: common,
            task,
        },
        endpoint,
    )
}

fn cloud_client(
    server: &Server,
    account: AuthCredentialIdentity,
    authorizer: Arc<AzureAdAuthorizer>,
) -> Arc<dyn LlmClient> {
    let auth_binding = AuthBindingRef {
        realm: RealmId::parse("ce-native").unwrap(),
        binding: BindingId::parse(format!("n1-route-{}", SessionId::new())).unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    };
    let backend = BackendProfile {
        id: "n1-dynamic-backend".into(),
        provider: Provider::OpenAI,
        backend_kind: "openai_api".into(),
        base_url: Some(server.base.clone()),
        options: Value::Null,
        server: None,
    };
    // AzureOpenAi only advertises azure_api_key today. This fixture instead
    // uses the supported OpenAiApi/external_authorizer host composition.
    let binding = ProviderRuntimeCatalog::validate_binding_with_credential_identity(
        &auth_binding,
        account,
        &backend,
        &AuthProfile {
            id: "n1-cloud-authorizer".into(),
            provider: Provider::OpenAI,
            auth_method: "external_authorizer".into(),
            source: CredentialSourceSpec::ExternalResolver {
                handle: "n1-host-cloud".into(),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
        &BindingPolicy::default(),
    )
    .unwrap();
    let identity = SessionLlmIdentity {
        model: MODEL.into(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(auth_binding),
    };
    let models =
        ModelRegistry::from_config(&Config::default(), meerkat_models::canonical()).unwrap();
    let profile = models
        .profile_witness_for_provider(Provider::OpenAI, MODEL)
        .unwrap();
    let connection = ResolvedConnection {
        provider: Provider::OpenAI,
        backend: NormalizedBackendKind::OpenAi(crate::OpenAiBackendKind::OpenAiApi),
        backend_profile: Arc::new(backend),
        credential_identity: binding.credential_identity().clone(),
        auth_lease: Arc::new(DynamicLease::from_authorizer(
            authorizer,
            AuthMetadata::default(),
            "n1-real-azure-ad",
        )),
    };
    ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(crate::OpenAiProviderRuntime))
        .build_text_client(ResolvedTextTarget::new(identity, profile, connection).unwrap())
        .unwrap()
}

async fn exercise_expiry(case: CredentialCase) {
    let unconfigured = MeerkatMachine::ephemeral();
    let authority = unconfigured.generated_auth_lease_handle();
    let account = unique_account();
    let lease = LeaseKey::from_credential_identity(&account);
    let (server, endpoint) = cloud_server(authority.clone(), lease.clone(), case).await;
    let mut cleanup = RefreshCaseCleanup {
        endpoint: server.endpoint.clone(),
        caller: None,
    };
    let authorizer = Arc::new(
        AzureAdAuthorizer::new(
            "https://cognitiveservices.azure.com/.default",
            AzureClientCredentials {
                tenant_id: "n1-tenant".into(),
                client_id: "n1-client".into(),
                client_secret: "n1-test-client-secret".into(),
                authority_host: server.base.clone(),
            },
        )
        .with_auth_lease_observer(authority.clone(), lease.clone())
        .with_token_url_override(format!("{}/token", server.base)),
    );
    // Initial credential acquisition only. After the Expired observation below,
    // the test never invokes authorize, prepare_request or a refresh function.
    let mut headers = Vec::new();
    authorizer
        .authorize(&mut HttpAuthorizationRequest {
            method: "POST",
            url: &format!("{}/v1/responses", server.base),
            headers: &mut headers,
        })
        .await
        .unwrap();
    assert_eq!(
        headers,
        [(
            "Authorization".to_string(),
            "Bearer n1-access-1".to_string()
        )]
    );
    let initial = authority.snapshot(&lease);
    assert_eq!(initial.phase, Some(AuthLeasePhase::Valid));
    assert!(initial.credential_present);
    assert_eq!(endpoint.token_forms.lock().unwrap().len(), 1);

    let client = cloud_client(&server, account.clone(), authorizer.clone());
    let selected = client.controller_model_selection().unwrap();
    assert_eq!(selected.credential(), &account);
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("n1-grants"),
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
            id("operation"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .unwrap();
    let expected_controller = controller.clone();
    let expected_operation = operation.clone();
    let expected_selection = selected.clone();
    // Actual current ingress permission owner. Revocation removes the exact
    // qualified requester row; it does not change credentials or grant ceilings.
    let admitted_requesters = Arc::new(Mutex::new(vec![principal("requester")]));
    let current_requesters = admitted_requesters.clone();
    let observed_endpoint = endpoint.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, input, current, association| {
        observed_endpoint
            .ingress_inputs
            .lock()
            .unwrap()
            .push(input.id().clone());
        let claim = association.candidate();
        if !current_requesters
            .lock()
            .unwrap()
            .contains(current.requester())
            || current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("ce-native").unwrap()
            || claim.logical_executor != principal("executor")
            || claim.represented_subject.is_some()
            || claim.target.logical_runtime != id(&runtime.to_string())
            || claim.controller_grant_lineage != [expected_controller.clone()]
            || claim.authority_basis
                != (WorkAuthorityBasis::GrantLineage {
                    lineage: vec![expected_operation.clone()],
                })
            || claim.controller_model.as_ref() != Some(&expected_selection)
            || claim.controller_ceiling != ceiling("infer")
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let machine = Arc::new(
        unconfigured
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                grants,
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(ResourceOwner {
                    selection: selected.clone(),
                    endpoint: format!("{}/v1/responses", server.base),
                }),
            })
            .unwrap(),
    );
    assert!(Arc::ptr_eq(
        &authority.clone_handle(),
        &machine.generated_auth_lease_handle().clone_handle()
    ));
    let tools = Arc::new(RecordingTools::default());
    let saved = Arc::new(RecordingStore::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(client);
    builder.default_tool_dispatcher = Some(tools.clone());
    builder.default_session_store = Some(saved.clone());
    let service = Arc::new(EphemeralSessionService::new(builder, 1));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: MODEL.into(),
            prompt: String::new().into(),
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
    let actor = service
        .live_session_actor_witness(&created.session_id)
        .await
        .unwrap();
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .unwrap();
    assert_eq!(pin.selection(), &selected);
    assert_eq!(
        LeaseKey::from_credential_identity(pin.selection().credential()),
        lease
    );

    let facts = pin.plain_facts().expect("actual selected controller facts");
    assert_eq!(facts.endpoint(), format!("{}/v1/responses", server.base));
    assert_eq!(
        authority.snapshot(&lease).expires_at,
        initial.expires_at,
        "session setup must retain the real token expiry"
    );
    *endpoint.native.lock().unwrap() = Some((Arc::downgrade(&machine), created.session_id.clone()));

    // Use the production status refresh window. A zero window overlaps the
    // generated Valid and Expired guards at exact second-resolution equality.
    // The real expires_in=0 token is observed using actual wall time.
    authority
        .observe_credential_freshness(
            &lease,
            Utc::now().timestamp().max(0) as u64,
            meerkat_core::handles::AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
        )
        .unwrap();
    if matches!(case, CredentialCase::Released) {
        authority.release_lease(&lease).unwrap();
    }
    let expected = match case {
        CredentialCase::Current => CredentialUseDisposition::Authorized,
        CredentialCase::Expired
        | CredentialCase::DeniedBeforeRefresh
        | CredentialCase::CancelDuringRefresh
        | CredentialCase::PolicyChangedDuringRefresh => CredentialUseDisposition::RefreshRequired,
        CredentialCase::Released => CredentialUseDisposition::LeaseAbsent,
    };
    assert_eq!(
        authority
            .resolve_credential_use_admission(&lease, CredentialUseIntent::HoldAuthority)
            .unwrap(),
        expected
    );
    if case.expired() {
        assert_eq!(
            authority.snapshot(&lease).phase,
            Some(AuthLeasePhase::Expired)
        );
    }
    assert_eq!(
        endpoint.token_forms.lock().unwrap().len(),
        1,
        "no test-owned refresh after status"
    );
    assert!(endpoint.model_bearers.lock().unwrap().is_empty());

    if matches!(
        case,
        CredentialCase::DeniedBeforeRefresh
            | CredentialCase::CancelDuringRefresh
            | CredentialCase::PolicyChangedDuringRefresh
    ) {
        let runtime = LogicalRuntimeId::for_session(&created.session_id);
        let mut prompt = PromptInput::new("Read record-7", None);
        prompt.header.authority_association = Some(association(
            &runtime,
            controller.clone(),
            operation.clone(),
            selected.clone(),
        ));
        let input = Input::Prompt(prompt);
        let input_id = input.id().clone();
        let ingress = NativeIngressContext::from_trusted_ingress(
            &input,
            principal("requester"),
            principal("ingress"),
            RealmId::parse("ce-native").unwrap(),
            evidence("current-authentication"),
        )
        .unwrap()
        .with_controller_client(&input, pin.clone())
        .unwrap();
        let input = input.with_ingress_context(ingress).unwrap();
        if matches!(case, CredentialCase::DeniedBeforeRefresh) {
            admitted_requesters.lock().unwrap().clear();
            let result = tokio::time::timeout(
                BOUND,
                machine.accept_input_with_completion(&created.session_id, input),
            )
            .await
            .expect("preflight denial is bounded");
            assert!(
                matches!(
                    &result,
                    Err(RuntimeDriverError::InputRefused { refusal })
                        if refusal.kind() == OperationRefusalKind::Denied
                ),
                "actual current ingress permission must refuse before refresh: {result:?}"
            );
            assert!(
                endpoint.ingress_inputs.lock().unwrap().contains(&input_id),
                "actual current ingress owner reached the denied requester row"
            );
            assert_eq!(endpoint.token_forms.lock().unwrap().len(), 1);
            assert_eq!(
                authority.snapshot(&lease).phase,
                Some(AuthLeasePhase::Expired)
            );
        } else {
            let admission_machine = machine.clone();
            let session = created.session_id.clone();
            let mut task = tokio::spawn(async move {
                admission_machine
                    .accept_input_with_completion(&session, input)
                    .await
            });
            cleanup.caller = Some(task.abort_handle());
            let arrived = tokio::time::timeout(BOUND, async {
                tokio::select! {
                    () = server.endpoint.refresh_arrived.notified() => Ok(()),
                    completed = &mut task => Err(match completed {
                        Ok(Err(error)) => format!("admission refused before refresh: {error}"),
                        Ok(Ok(_)) => "input accepted before required refresh HTTP".into(),
                        Err(error) => format!("admission task failed before refresh: {error}"),
                    }),
                }
            })
            .await;
            match arrived {
                Ok(Ok(())) => {}
                Ok(Err(error)) => panic!("{error}"),
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    server.endpoint.refresh_release.add_permits(1);
                    panic!("actual refresh HTTP did not arrive within bound");
                }
            }
            assert_eq!(endpoint.token_forms.lock().unwrap().len(), 2);
            assert_eq!(
                authority.snapshot(&lease).phase,
                Some(AuthLeasePhase::Refreshing)
            );
            assert!(
                machine
                    .input_state(&created.session_id, &input_id)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(server.endpoint.model_requests.lock().unwrap().is_empty());
            assert!(tools.0.lock().unwrap().is_empty());
            if matches!(case, CredentialCase::CancelDuringRefresh) {
                task.abort();
                assert!(matches!(task.await, Err(error) if error.is_cancelled()));
                // The caller is gone while the real exchange is still gated.
                assert!(
                    machine
                        .input_state(&created.session_id, &input_id)
                        .await
                        .unwrap()
                        .is_none()
                );
                server.endpoint.refresh_release.add_permits(1);
                // This takes the actual authorizer's refresh lock after the
                // owned maintenance. A canceled maintenance would require a
                // third physical exchange and fails the exact count below.
                tokio::time::timeout(BOUND, authorizer.prepare_request())
                    .await
                    .expect("owned refresh settles within bound")
                    .unwrap();
            } else {
                admitted_requesters
                    .lock()
                    .unwrap()
                    .retain(|row| row != &principal("requester"));
                server.endpoint.refresh_release.add_permits(1);
                let result = tokio::time::timeout(BOUND, task)
                    .await
                    .expect("final current-policy check is bounded")
                    .unwrap();
                assert!(
                    matches!(
                        &result,
                        Err(RuntimeDriverError::InputRefused { refusal })
                            if refusal.kind() == OperationRefusalKind::Denied
                    ),
                    "requester removed during HTTP must fail final native admission: {result:?}"
                );
                assert!(
                    endpoint
                        .ingress_inputs
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|observed| *observed == &input_id)
                        .count()
                        >= 2,
                    "actual ingress owner runs before HTTP and at final admission"
                );
            }
            assert_eq!(
                endpoint.token_forms.lock().unwrap().len(),
                2,
                "only the owned refresh exchanges the replacement token"
            );
            assert_eq!(
                authority.snapshot(&lease).phase,
                Some(AuthLeasePhase::Valid)
            );
        }
        assert!(
            machine
                .input_state(&created.session_id, &input_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(server.endpoint.model_requests.lock().unwrap().is_empty());
        assert!(tools.0.lock().unwrap().is_empty());
        *admitted_requesters.lock().unwrap() = vec![principal("requester")];
        // Restore the exact owner row for a real permitted continuation control.
        // The canceled or denied input remains absent after that work completes.
        server.endpoint.refresh_release.add_permits(1);
        tokio::time::timeout(
            BOUND,
            run_native(
                machine.clone(),
                server.endpoint.clone(),
                saved,
                created.session_id.clone(),
                pin,
                controller,
                operation,
                tools.clone(),
            ),
        )
        .await
        .expect("permitted input completes after the refused/canceled attempt");
        assert!(
            machine
                .input_state(&created.session_id, &input_id)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(endpoint.token_forms.lock().unwrap().len(), 2);
        assert_eq!(server.endpoint.model_requests.lock().unwrap().len(), 2);
        assert_eq!(tools.0.lock().unwrap().len(), 1);
        assert_eq!(
            *endpoint.model_bearers.lock().unwrap(),
            vec!["Bearer n1-access-2"; 2]
        );
        return;
    }

    if matches!(case, CredentialCase::Released) {
        let runtime = LogicalRuntimeId::for_session(&created.session_id);
        let mut prompt = PromptInput::new("Read record-7", None);
        prompt.header.authority_association =
            Some(association(&runtime, controller, operation, selected));
        let input = Input::Prompt(prompt);
        let input_id = input.id().clone();
        let ingress = NativeIngressContext::from_trusted_ingress(
            &input,
            principal("requester"),
            principal("ingress"),
            RealmId::parse("ce-native").unwrap(),
            evidence("current-authentication"),
        )
        .unwrap()
        .with_controller_client(&input, pin)
        .unwrap();
        let result = tokio::time::timeout(
            BOUND,
            machine.accept_input_with_completion(
                &created.session_id,
                input.with_ingress_context(ingress).unwrap(),
            ),
        )
        .await
        .expect("unusable rejection is bounded");
        assert!(
            matches!(
                result,
                Err(RuntimeDriverError::ControllerReadinessUnavailable {
                    reason: ControllerReadinessFailure::CredentialUnusable {
                        disposition: CredentialUseDisposition::LeaseAbsent
                    }
                })
            ),
            "released credential must remain unusable"
        );
        assert!(
            machine
                .input_state(&created.session_id, &input_id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(endpoint.model_bearers.lock().unwrap().is_empty());
        assert!(tools.0.lock().unwrap().is_empty());
        assert_eq!(
            endpoint.token_forms.lock().unwrap().len(),
            1,
            "released authority cannot trigger credential resurrection"
        );
        assert_eq!(
            authority
                .resolve_credential_use_admission(&lease, CredentialUseIntent::HoldAuthority)
                .unwrap(),
            CredentialUseDisposition::LeaseAbsent
        );
        return;
    }

    // No manual refresh, ungoverned helper request or test-owned retry. The real
    // governed path must reach the actual refresh owner and complete normally.
    let _observed = tokio::time::timeout(
        BOUND,
        run_native(
            machine.clone(),
            server.endpoint.clone(),
            saved,
            created.session_id,
            pin,
            controller,
            operation,
            tools.clone(),
        ),
    )
    .await
    .expect("governed current/refreshable request must progress within bound");
    let expected_tokens = if case.expired() { 2 } else { 1 };
    let forms = endpoint.token_forms.lock().unwrap();
    assert_eq!(
        forms.len(),
        expected_tokens,
        "case={case:?}: real token HTTP count"
    );
    for form in forms.iter() {
        assert_eq!(form.len(), 4);
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some("client_credentials")
        );
        assert_eq!(form.get("client_id").map(String::as_str), Some("n1-client"));
        assert_eq!(
            form.get("client_secret").map(String::as_str),
            Some("n1-test-client-secret")
        );
        assert_eq!(
            form.get("scope").map(String::as_str),
            Some("https://cognitiveservices.azure.com/.default")
        );
    }
    drop(forms);
    assert_eq!(
        *endpoint.model_bearers.lock().unwrap(),
        vec![format!("Bearer n1-access-{expected_tokens}"); 2]
    );
    assert_eq!(
        *endpoint.model_authority.lock().unwrap(),
        vec![CredentialUseDisposition::Authorized; 2]
    );
    assert_eq!(tools.0.lock().unwrap().len(), 1);
    assert_eq!(server.endpoint.model_requests.lock().unwrap().len(), 2);
    let final_state = authority.snapshot(&lease);
    assert_eq!(final_state.phase, Some(AuthLeasePhase::Valid));
    assert!(final_state.credential_present);
    if case.expired() {
        assert_eq!(*endpoint.refresh_before_admission.lock().unwrap(), [true]);
        assert_eq!(
            *endpoint.refresh_phases.lock().unwrap(),
            [Some(AuthLeasePhase::Refreshing)]
        );
        assert!(
            final_state.generation > initial.generation,
            "actual refresh must advance the generated owner"
        );
        assert!(final_state.expires_at.unwrap() > Utc::now().timestamp().max(0) as u64);
    } else {
        assert_eq!(final_state.generation, initial.generation);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n1_current_dynamic_controller_completes_without_refresh() {
    exercise_expiry(CredentialCase::Current).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n1_expired_dynamic_controller_reaches_real_refresh_and_completes() {
    exercise_expiry(CredentialCase::Expired).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n1_released_dynamic_controller_cannot_refresh_or_enter() {
    exercise_expiry(CredentialCase::Released).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n1_disallowed_requester_does_not_refresh_before_admission() {
    exercise_expiry(CredentialCase::DeniedBeforeRefresh).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n1_waiter_abort_retains_only_refresh_and_never_accepts_input() {
    exercise_expiry(CredentialCase::CancelDuringRefresh).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n1_requester_revoked_during_refresh_is_rechecked_before_admission() {
    exercise_expiry(CredentialCase::PolicyChangedDuringRefresh).await;
}
