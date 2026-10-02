//! Actual native progress in a different existing session during real OAuth HTTP.
//! The model HTTP authorizer is a fixture, not proof of old-token wire continuity.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::{OpenAiOAuthRuntime, TokenPrepareFn, chatgpt_endpoints};
use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use chrono::Utc;
use meerkat::{AgentFactory, EphemeralSessionService, FactoryAgentBuilder};
use meerkat_auth_core::resolver::{
    load_managed_store_tokens_with_lifecycle, prepare_managed_store_oauth_refresh_under_lock,
};
use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, ControllerAdmissionAllowance, OperationPolicyOwner, WorkOwnerAllowance,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization::work::HostAuthorizationClock;
use meerkat_authorization_contracts::audit::{
    AuditObservation, AuditTarget, StoredAuthorizationAuditObservation,
};
use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, ExactRestriction, ExecutionRestrictions, ProcessorRef, ResourceDomain,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
    NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
};
use meerkat_core::agent::ToolDispatchContext;
use meerkat_core::auth::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, RefreshError, TokenKey, TokenStore,
};
use meerkat_core::authorization::{
    AuthorizationOperation, OperationObservedOutcome, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, ToolAuthorizationTarget,
};
use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::handles::{
    AuthLeasePhase, CredentialUseDisposition, CredentialUseIntent, GeneratedAuthLeaseHandle,
    LeaseKey,
};
use meerkat_core::service::{CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::{
    AgentError, AgentSessionStore, AgentToolDispatcher, AuthBindingRef, AuthMetadata, AuthProfile,
    BackendProfile, BindingId, BindingOrigin, BindingPolicy, Config, ControllerModelClient,
    ControllerModelSelection, CredentialSourceSpec, HttpAuthorizer, Message, ModelRegistry,
    OperationAuthorizationError, PrincipalKind, PrincipalRef, Provider, Session, SessionId,
    SessionLlmIdentity, ToolCallView, ToolDef, ToolDispatchOutcome, ToolError, ToolResult,
    TrustDomainId,
};
use meerkat_llm_core::LlmClient;
use meerkat_llm_core::provider_runtime::binding::{
    DynamicLease, NormalizedBackendKind, ResolvedConnection, ResolvedTextTarget,
};
use meerkat_llm_core::provider_runtime::registry::{ProviderRuntimeRegistry, ResolverEnvironment};
use meerkat_llm_core::provider_runtime::{ProviderRuntimeCatalog, ValidatedBinding};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_runtime::input_authority::NativeIngressContext;
use meerkat_runtime::meerkat_machine::{
    MeerkatMachine, NativeGrantWorkConfiguration, NativeIngressCheck,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};

const BOUND: Duration = Duration::from_secs(15);
const MODEL: &str = "gpt-5.5";
const CALL: &str = "ce-read";
const FINISHED: &str = "the permitted record was read";

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).unwrap()
}
fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("ce-native").unwrap(),
    )
    .unwrap()
}
fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}
fn unique_account() -> AuthCredentialIdentity {
    serde_json::from_value(
        json!({"realm":"ce-native", "account":format!("account-x-{}", SessionId::new())}),
    )
    .unwrap()
}
fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: principal("ingress"),
                namespace: "observations".into(),
            },
            resource_id: value.into(),
        },
        revision: id("observation-1"),
        digest: EvidenceDigest::from_array([1; 32]),
    }
}
fn action(value: &str) -> ActionRef {
    ActionRef {
        feature: "ce-native".into(),
        action: value.into(),
    }
}
fn domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("resource-owner"),
        namespace: "records".into(),
    }
}
fn values(verb: &str) -> Vec<LocalOperationValues> {
    vec![LocalOperationValues {
        action: action(verb),
        resource_domain: domain(),
        processor: ProcessorRef::Principal {
            principal: principal("executor"),
        },
        audience: AudienceRef::Principal {
            principal: principal("requester"),
        },
    }]
}
fn ceiling(verb: &str) -> ExecutionRestrictions {
    let mut ceiling = ExecutionRestrictions::unrestricted();
    ceiling.actions = ExactRestriction::exact([action(verb)]);
    ceiling.resource_domains = ExactRestriction::exact([domain()]);
    ceiling.processors = ExactRestriction::exact([ProcessorRef::Principal {
        principal: principal("executor"),
    }]);
    ceiling.audiences = ExactRestriction::exact([AudienceRef::Principal {
        principal: principal("requester"),
    }]);
    ceiling
}
struct InvocationOwner;
impl AdmittedWorkPolicyOwner for InvocationOwner {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        now: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        let claim = association.candidate();
        if claim.requester != principal("requester")
            || claim.ingress_actor != principal("ingress")
            || claim.logical_executor != principal("executor")
            || claim.represented_subject.is_some()
        {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now + 60_000,
        })
    }
}
struct ResourceOwner {
    selection: ControllerModelSelection,
    endpoint: String,
}
impl OperationPolicyOwner for ResourceOwner {
    fn authorize_controller_admission(
        &self,
        claim: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        _: u64,
    ) -> Result<ControllerAdmissionAllowance, OperationAuthorizationError> {
        if claim.candidate().controller_model.as_ref() != Some(&self.selection)
            || facts.selection() != &self.selection
            || facts.endpoint() != self.endpoint
            || facts.wire_model() != MODEL
        {
            return Err(denied().into());
        }
        Ok(ControllerAdmissionAllowance {
            operation_values: values("infer"),
            restrictions: ExecutionRestrictions::unrestricted(),
        })
    }
    fn authorize_operation(
        &self,
        _: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        let verb = match &binding.facts().operation {
            AuthorizationOperation::Model(facts)
                if purpose == LocalPolicyPurpose::Controller
                    && self.selection.matches_model_facts(facts)
                    && facts.endpoint.as_ref() == self.endpoint
                    && facts.wire_model.as_ref() == MODEL
                    && facts.hosted_capabilities.is_empty()
                    && facts.live_channel.is_none() =>
            {
                "infer"
            }
            AuthorizationOperation::Tool(facts)
                if purpose == LocalPolicyPurpose::Operation
                    && facts.name == "read_record"
                    && matches!(facts.target, ToolAuthorizationTarget::Dispatcher(_)) =>
            {
                let args: Value =
                    serde_json::from_str(facts.arguments.get()).map_err(|_| denied())?;
                if args != json!({"record":"record-7"}) {
                    return Err(denied().into());
                }
                "read"
            }
            _ => return Err(denied().into()),
        };
        Ok(LocalPolicyAllowance {
            operation_values: values(verb),
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now + 60_000,
        })
    }
}
#[derive(Default)]
struct RecordingStore(Mutex<HashMap<SessionId, Session>>);
#[async_trait]
impl AgentSessionStore for RecordingStore {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.0
            .lock()
            .unwrap()
            .insert(session.id().clone(), session.clone());
        Ok(())
    }
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        Ok(SessionId::parse(id)
            .ok()
            .and_then(|id| self.0.lock().unwrap().get(&id).cloned()))
    }
}
#[derive(Default)]
struct RecordingTools(Mutex<Vec<String>>);
#[async_trait]
impl AgentToolDispatcher for RecordingTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(ToolDef::new("read_record", "Read a fixture record", json!({"type":"object","properties":{"record":{"type":"string"}},"required":["record"],"additionalProperties":false})))].into()
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.0.lock().unwrap().push(call.id.into());
        Ok(ToolResult::new(call.id.into(), "record-7 value".into(), false).into())
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        assert!(
            context.work_authorization().is_some(),
            "actual native work reaches the dispatched tool"
        );
        self.dispatch(call).await
    }
}

struct Endpoint {
    model_requests: Mutex<Vec<Value>>,
    model_result_arrived: Notify,
    model_finish: Semaphore,
    refresh_arrived: Notify,
    refresh_release: Semaphore,
    refresh_requests: Mutex<Vec<Vec<u8>>>,
}
struct Server {
    base: String,
    endpoint: Arc<Endpoint>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.endpoint.refresh_release.close();
        self.endpoint.model_finish.close();
        self.task.abort();
    }
}
impl Server {
    async fn start() -> Self {
        let endpoint = Arc::new(Endpoint {
            model_requests: Mutex::new(Vec::new()),
            model_result_arrived: Notify::new(),
            model_finish: Semaphore::new(0),
            refresh_arrived: Notify::new(),
            refresh_release: Semaphore::new(0),
            refresh_requests: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/responses", post(model))
            .route("/token", post(token))
            .with_state(endpoint.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base,
            endpoint,
            task,
        }
    }
}
async fn token(State(endpoint): State<Arc<Endpoint>>, body: axum::body::Bytes) -> Response {
    endpoint
        .refresh_requests
        .lock()
        .unwrap()
        .push(body.to_vec());
    endpoint.refresh_arrived.notify_one();
    match endpoint.refresh_release.acquire().await {
        Ok(permit) => permit.forget(),
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
    Json(json!({"access_token":"ce-native-access-b", "refresh_token":"ce-native-refresh-b", "expires_in":3600})).into_response()
}
async fn model(
    State(endpoint): State<Arc<Endpoint>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer ce-native-model-fixture")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let has_result = body["input"].as_array().is_some_and(|input| {
        input
            .iter()
            .any(|item| item["type"] == "function_call_output")
    });
    endpoint.model_requests.lock().unwrap().push(body);
    let output = if has_result {
        endpoint.model_result_arrived.notify_one();
        match endpoint.model_finish.acquire().await {
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
struct ModelAuthorizer;
#[async_trait]
impl HttpAuthorizer for ModelAuthorizer {
    async fn authorize(
        &self,
        request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
    ) -> Result<(), meerkat_core::AuthError> {
        request.headers.push((
            "Authorization".into(),
            "Bearer ce-native-model-fixture".into(),
        ));
        Ok(())
    }
    fn label(&self) -> &str {
        "ce-native-model-fixture"
    }
}
fn binding(server: &Server) -> ValidatedBinding {
    let binding = AuthBindingRef {
        realm: RealmId::parse("ce-native").unwrap(),
        binding: BindingId::parse(format!("route-{}", SessionId::new())).unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    };
    ProviderRuntimeCatalog::validate_binding_with_credential_identity(
        &binding,
        unique_account(),
        &BackendProfile {
            id: "ce-native-backend".into(),
            provider: Provider::OpenAI,
            backend_kind: "chatgpt_backend".into(),
            base_url: Some(server.base.clone()),
            options: Value::Null,
            server: None,
        },
        &AuthProfile {
            id: "ce-native-oauth".into(),
            provider: Provider::OpenAI,
            auth_method: "managed_chatgpt_oauth".into(),
            source: CredentialSourceSpec::ManagedStore,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
        &BindingPolicy::default(),
    )
    .unwrap()
}
fn model_client(server: &Server, binding: &ValidatedBinding) -> Arc<dyn LlmClient> {
    let identity = SessionLlmIdentity {
        model: MODEL.into(),
        provider: Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: Some(binding.auth_binding_ref().clone()),
    };
    let models =
        ModelRegistry::from_config(&Config::default(), meerkat_models::canonical()).unwrap();
    let profile = models
        .profile_witness_for_provider(Provider::OpenAI, MODEL)
        .unwrap();
    let connection = ResolvedConnection {
        provider: Provider::OpenAI,
        backend: NormalizedBackendKind::OpenAi(crate::OpenAiBackendKind::ChatGptBackend),
        backend_profile: Arc::new(BackendProfile {
            id: "ce-native-backend".into(),
            provider: Provider::OpenAI,
            backend_kind: "chatgpt_backend".into(),
            base_url: Some(server.base.clone()),
            options: Value::Null,
            server: None,
        }),
        credential_identity: binding.credential_identity().clone(),
        auth_lease: Arc::new(DynamicLease::from_authorizer(
            Arc::new(ModelAuthorizer),
            AuthMetadata::default(),
            "ce-native-model-fixture",
        )),
    };
    ProviderRuntimeRegistry::empty()
        .with_runtime(Arc::new(crate::OpenAiProviderRuntime))
        .build_text_client(ResolvedTextTarget::new(identity, profile, connection).unwrap())
        .unwrap()
}

fn association(
    runtime: &LogicalRuntimeId,
    controller: GrantLineageRef,
    operation: GrantLineageRef,
    selected: ControllerModelSelection,
) -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: principal("requester"),
        ingress_actor: principal("ingress"),
        represented_subject: None,
        original_authentication: evidence("original-authentication"),
        logical_executor: principal("executor"),
        target: NativeWorkTarget {
            logical_owner: principal("native-owner"),
            logical_runtime: id(&runtime.to_string()),
            context: id("native-context"),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: principal("requester"),
            },
        },
        original_work: OriginalWorkRef {
            authority: principal("ingress"),
            work: id(&runtime.to_string()),
        },
        root_event: evidence("original-event"),
        contributing_work: Vec::new(),
        authority_basis: WorkAuthorityBasis::GrantLineage {
            lineage: vec![operation],
        },
        controller_grant_lineage: vec![controller],
        controller_model: Some(selected),
        controller_ceiling: ceiling("infer"),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: RealmId::parse("ce-native").expect("realm"),
            ingress: ResourceDomain {
                authority: principal("ingress"),
                namespace: "prompts".into(),
            },
            occurrence_scope: id(&runtime.to_string()),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .expect("immutable association claims")
}

struct ObservedRun {
    input: meerkat_core::InputId,
    run: meerkat_core::RunId,
}
async fn run_native(
    machine: Arc<MeerkatMachine>,
    endpoint: Arc<Endpoint>,
    saved: Arc<RecordingStore>,
    session: SessionId,
    pin: ControllerModelClient,
    controller: GrantLineageRef,
    operation: GrantLineageRef,
    tools: Arc<RecordingTools>,
) -> ObservedRun {
    let before_tools = tools.0.lock().unwrap().len();
    let before_http = endpoint.model_requests.lock().unwrap().len();
    let runtime = LogicalRuntimeId::for_session(&session);
    let claims = association(&runtime, controller, operation, pin.selection().clone());
    let mut prompt = PromptInput::new("Read record-7 using read_record, then finish", None);
    prompt.header.authority_association = Some(claims);
    let input = Input::Prompt(prompt);
    let id = input.id().clone();
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
    let (accepted, completion) = machine
        .accept_input_with_completion(&session, input.with_ingress_context(ingress).unwrap())
        .await
        .unwrap();
    assert!(
        matches!(accepted, meerkat_runtime::accept::AcceptOutcome::Accepted { input_id, .. } if input_id == id)
    );
    let completion = completion.expect("real native completion receipt");
    tokio::time::timeout(BOUND, endpoint.model_result_arrived.notified())
        .await
        .expect("actual second model request includes tool result");
    let bodies = endpoint.model_requests.lock().unwrap().clone();
    assert_eq!(
        bodies.len(),
        before_http + 2,
        "one real model-tool-model pair"
    );
    let last = bodies.last().unwrap();
    assert_eq!(last["model"], MODEL);
    let outputs: Vec<_> = last["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0]["call_id"], CALL);
    assert_eq!(outputs[0]["output"], "record-7 value");
    assert_eq!(
        tools.0.lock().unwrap().len(),
        before_tools + 1,
        "actual permitted tool body entered once"
    );
    let stored = machine
        .input_state(&session, &id)
        .await
        .unwrap()
        .expect("actual running native row");
    let audit: Vec<StoredAuthorizationAuditObservation> = serde_json::from_value(
        serde_json::to_value(&stored).unwrap()["authorization_audit"].clone(),
    )
    .unwrap();
    let run = audit.first().unwrap().observation.run_id.clone().unwrap();
    for record in &audit {
        assert_eq!(record.contributors.len(), 1);
        assert_eq!(record.contributors[0].input_id, id);
        assert_eq!(record.observation.run_id.as_ref(), Some(&run));
        assert!(
            matches!(&record.observation.execution_scope, OperationExecutionScope::RuntimeInput { owner_session_id, submitted_input_id, canonical_input_id, .. } if owner_session_id == &session && submitted_input_id == &id && canonical_input_id == &id)
        );
    }
    let prepared = audit.iter().find(|record| matches!(&record.observation.observation, AuditObservation::Prepared { target, .. } if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. } if call_id == CALL && tool_name == "read_record"))).unwrap();
    assert!(audit.iter().any(|record| record.observation.operation_id
        == prepared.observation.operation_id
        && matches!(record.observation.observation, AuditObservation::Entry)));
    assert!(audit.iter().any(|record| record.observation.operation_id
        == prepared.observation.operation_id
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
    endpoint.model_finish.add_permits(1);
    let outcome = tokio::time::timeout(BOUND, completion.wait())
        .await
        .unwrap()
        .unwrap();
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("native run did not complete normally: {outcome:?}");
    };
    assert_eq!(result.text, FINISHED);
    assert_eq!(result.session_id, session);
    assert!(result.terminal_cause_kind.is_none());
    let retained = saved.0.lock().unwrap().get(&session).cloned().unwrap();
    assert!(retained.messages().iter().any(|message| matches!(message, Message::ToolResults { results, .. } if results.iter().any(|result| result.tool_use_id == CALL && !result.is_error && result.text_content() == "record-7 value"))));
    ObservedRun { input: id, run }
}

async fn exercise(paused_refresh: bool) {
    let server = Server::start().await;
    let binding = binding(&server);
    // A fresh key isolates the process-global lifecycle lock between tests.
    // Both native actors and OAuth retain this one exact Account identity.
    let account = binding.credential_identity().clone();
    assert!(matches!(
        binding.credential_identity(),
        AuthCredentialIdentity::Account(_)
    ));
    assert_eq!(binding.credential_identity(), &account);
    let client = model_client(&server, &binding);
    let selected = client
        .controller_model_selection()
        .expect("actual provider registry selected route");
    assert_eq!(selected.credential(), &account);
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("ce-grants"),
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
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, association| {
        let candidate = association.candidate();
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("ce-native").unwrap()
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
                operation_owner: Arc::new(ResourceOwner {
                    selection: selected.clone(),
                    endpoint: format!("{}/responses", server.base),
                }),
            })
            .unwrap(),
    );
    let tools = Arc::new(RecordingTools::default());
    let saved = Arc::new(RecordingStore::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(client);
    builder.default_tool_dispatcher = Some(tools.clone());
    builder.default_session_store = Some(saved.clone());
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let mut attached = Vec::new();
    for _ in 0..2 {
        let created = meerkat::surface::materialize_ephemeral_runtime_session(
            &service,
            &machine,
            CreateSessionRequest {
                model: MODEL.into(),
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
        let actor = service
            .live_session_actor_witness(&created.session_id)
            .await
            .unwrap();
        let pin = service
            .pin_controller_client_for_actor(&actor)
            .await
            .unwrap();
        assert_eq!(pin.selection(), &selected);
        assert!(matches!(
            pin.selection().credential(),
            AuthCredentialIdentity::Account(_)
        ));
        assert_eq!(pin.selection().credential(), &account);
        assert_eq!(
            LeaseKey::from_credential_identity(pin.selection().credential()),
            LeaseKey::from_credential_identity(binding.credential_identity())
        );
        attached.push((created.session_id, pin));
    }
    let (session_b, pin_b) = attached.pop().unwrap();
    let (session_a, pin_a) = attached.pop().unwrap();
    assert_ne!(session_a, session_b);
    assert_ne!(
        LogicalRuntimeId::for_session(&session_a),
        LogicalRuntimeId::for_session(&session_b)
    );
    let vault = Arc::new(EphemeralTokenStore::new());
    let persistence =
        ProviderAuthPersistence::new(vault.clone(), Arc::new(InMemoryCoordinator::new()));
    let authority: GeneratedAuthLeaseHandle = machine.generated_auth_lease_handle();
    let key = TokenKey::from_credential_identity(binding.credential_identity());
    let lease = LeaseKey::from_credential_identity(binding.credential_identity());
    let raw = PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("ce-native-access-a".into()),
        refresh_token: Some("ce-native-refresh-a".into()),
        id_token: None,
        expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: Vec::new(),
        account_id: Some(account.account().unwrap().to_string()),
        metadata: Value::Null,
    };
    meerkat_auth_core::save_tokens_and_publish_lifecycle(
        persistence.clone(),
        authority.clone(),
        account.clone(),
        raw,
    )
    .await
    .unwrap();
    let original = vault.load(&key).await.unwrap().unwrap();
    assert_eq!(
        authority
            .resolve_credential_use_admission(&lease, CredentialUseIntent::HoldAuthority)
            .unwrap(),
        CredentialUseDisposition::Authorized
    );
    let a = run_native(
        machine.clone(),
        server.endpoint.clone(),
        saved.clone(),
        session_a.clone(),
        pin_a,
        controller.clone(),
        operation.clone(),
        tools.clone(),
    )
    .await;
    // Both actors existed before the refresh; A has also executed a normal run.
    service
        .live_session_actor_witness(&session_a)
        .await
        .unwrap();
    service
        .live_session_actor_witness(&session_b)
        .await
        .unwrap();
    assert_eq!(tools.0.lock().unwrap().len(), 1);
    assert_eq!(server.endpoint.model_requests.lock().unwrap().len(), 2);

    let mut refresh = if paused_refresh {
        assert!(
            Arc::ptr_eq(
                &authority.clone_handle(),
                &machine.generated_auth_lease_handle().clone_handle()
            ),
            "OAuth and native admission use the same actual generated owner"
        );
        let env = ResolverEnvironment::testing()
            .with_provider_auth_persistence(persistence.clone())
            .with_auth_lease_handle(authority.clone())
            .with_force_refresh(true);
        let mut previous = load_managed_store_tokens_with_lifecycle(&env, &binding)
            .await
            .unwrap();
        previous.release_prelock_lifecycle_guard();
        let prepare_binding = binding.clone();
        let prepare: TokenPrepareFn = Box::new(move |locked, mode| {
            Box::pin(async move {
                prepare_managed_store_oauth_refresh_under_lock(
                    &env,
                    &prepare_binding,
                    previous,
                    locked,
                    mode,
                )
                .await
                .map_err(|error| RefreshError::Refresh(error.to_string()))
            })
        });
        let mut endpoints = chatgpt_endpoints("http://127.0.0.1:0/callback");
        endpoints.token_url = format!("{}/token", server.base);
        let runtime = OpenAiOAuthRuntime::new(persistence, endpoints, key.clone());
        let task = tokio::spawn(async move {
            runtime
                .refresh_tokens_with_locked_preparation(prepare, true)
                .await
        });
        if tokio::time::timeout(BOUND, server.endpoint.refresh_arrived.notified())
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
            panic!("actual OAuth HTTP must reach the paused endpoint");
        }
        assert_eq!(
            authority.snapshot(&lease).phase,
            Some(AuthLeasePhase::Refreshing)
        );
        assert_eq!(vault.load(&key).await.unwrap(), Some(original.clone()));
        assert_eq!(server.endpoint.refresh_release.available_permits(), 0);
        assert!(!task.is_finished());
        Some(task)
    } else {
        None
    };

    let b_session = session_b.clone();
    let mut work = tokio::spawn(run_native(
        machine.clone(),
        server.endpoint.clone(),
        saved.clone(),
        b_session,
        pin_b,
        controller,
        operation,
        tools.clone(),
    ));
    let progress = tokio::time::timeout(BOUND, &mut work).await;
    let completed_while_paused = progress.is_ok();
    // Release/settle the actual HTTP owner before failing the liveness oracle.
    // Success after this point is cleanup, never evidence of paused progress.
    if let Some(task) = refresh.as_ref() {
        assert!(
            !task.is_finished(),
            "OAuth response gate was not released by native work"
        );
        assert_eq!(server.endpoint.refresh_release.available_permits(), 0);
    }
    server.endpoint.refresh_release.add_permits(1);
    let refresh_result = if let Some(mut task) = refresh.take() {
        match tokio::time::timeout(BOUND, &mut task).await {
            Ok(result) => Some(result),
            Err(_) => {
                task.abort();
                let _ = task.await;
                None
            }
        }
    } else {
        None
    };
    let b = match progress {
        Ok(result) => result,
        Err(_) => match tokio::time::timeout(BOUND, &mut work).await {
            Ok(result) => result,
            Err(_) => {
                work.abort();
                let _ = work.await;
                panic!("native work did not settle after OAuth cleanup");
            }
        },
    }
    .expect("native worker did not panic");
    assert!(
        completed_while_paused,
        "different existing session must admit and execute its real tool/model result before OAuth response release"
    );
    assert_ne!(a.input, b.input);
    assert_ne!(a.run, b.run);
    assert_eq!(tools.0.lock().unwrap().as_slice(), [CALL, CALL]);
    assert_eq!(
        server.endpoint.model_requests.lock().unwrap().len(),
        4,
        "no blind model/tool retry"
    );
    if paused_refresh {
        let refreshed = refresh_result
            .expect("refresh completion bound")
            .expect("refresh task")
            .expect("actual rotated response committed");
        assert_eq!(
            refreshed.primary_secret.as_deref(),
            Some("ce-native-access-b")
        );
        assert_eq!(
            refreshed.refresh_token.as_deref(),
            Some("ce-native-refresh-b")
        );
        assert_eq!(vault.load(&key).await.unwrap(), Some(refreshed));
        assert_eq!(
            authority
                .resolve_credential_use_admission(&lease, CredentialUseIntent::HoldAuthority)
                .unwrap(),
            CredentialUseDisposition::Authorized
        );
        let requests = server.endpoint.refresh_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let form = String::from_utf8(requests[0].clone()).unwrap();
        assert!(
            form.contains("grant_type=refresh_token")
                && form.contains("refresh_token=ce-native-refresh-a")
        );
    } else {
        assert!(server.endpoint.refresh_requests.lock().unwrap().is_empty());
        assert_eq!(vault.load(&key).await.unwrap(), Some(original));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ce_native_two_existing_sessions_same_account_positive() {
    exercise(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ce_native_other_existing_session_progresses_during_oauth_http() {
    exercise(true).await;
}

#[path = "ce_native_progress/n1_expired_admission.rs"]
mod n1_expired_admission;
