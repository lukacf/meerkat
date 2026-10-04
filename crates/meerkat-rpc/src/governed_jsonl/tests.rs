//! End-to-end fixtures for fixed commissioned JSONL admission and tool feedback.
//! The private HTTP provider is the accepted E1 receiver, not a live model.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use async_trait::async_trait;
use futures::StreamExt;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance,
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
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse,
    OperationObservedOutcome, OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
    ToolAuthorizationTarget,
};
use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::service::SessionService;
use meerkat_core::service::{CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::{
    AgentError, AgentSessionStore, AgentToolDispatcher, Config, ControllerModelSelection, Message,
    PrincipalKind, PrincipalRef, Provider, Session, SessionId, SessionLlmIdentity, StopReason,
    ToolCallView, ToolDef, ToolDispatchOutcome, ToolError, ToolResult, TrustDomainId,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PreparedLlmRequest,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_runtime::input_authority::NativeIngressContext;
use meerkat_runtime::meerkat_machine::{
    MeerkatMachine, NativeGrantWorkConfiguration, NativeIngressCheck,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use meerkat_runtime::store::{InMemoryRuntimeStore, RuntimeStore};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Notify;

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

const DENIED_CALL: &str = "attempt-delete";
const PERMITTED_CALL: &str = "attempt-read";
fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded fixture identifier")
}

fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("native-loop").expect("domain"),
    )
    .expect("qualified fixture principal")
}

fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
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
        feature: "native-loop".into(),
        action: value.into(),
    }
}

fn domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("resource-owner"),
        namespace: "records".into(),
    }
}

fn ceiling(action_name: &str) -> ExecutionRestrictions {
    let mut bounds = ExecutionRestrictions::unrestricted();
    bounds.actions = ExactRestriction::exact([action(action_name)]);
    bounds.resource_domains = ExactRestriction::exact([domain()]);
    bounds.processors = ExactRestriction::exact([ProcessorRef::Principal {
        principal: principal("executor"),
    }]);
    bounds.audiences = ExactRestriction::exact([AudienceRef::Principal {
        principal: principal("requester"),
    }]);
    bounds
}

/// Configured application entitlement only. NativeAcceptedWorkOwner still
/// proves exact retained rows/run custody before invoking this application hook,
/// and GrantBackedWorkPolicy resolves the actual generated grant separately.
struct InvocationOwner;
impl AdmittedWorkPolicyOwner for InvocationOwner {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        let candidate = association.candidate();
        if candidate.requester != principal("requester")
            || candidate.ingress_actor != principal("ingress")
            || candidate.logical_executor != principal("executor")
            || candidate.represented_subject.is_some()
        {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }
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
            work: id("work-7"),
        },
        root_event: evidence("original-event"),
        contributing_work: Vec::new(),
        authority_basis: WorkAuthorityBasis::GrantLineage {
            lineage: vec![operation],
        },
        controller_grant_lineage: vec![controller],
        controller_model: Some(selected),
        controller_ceiling: ceiling("infer"),
        // The operation's admitted ceiling is broad enough to map delete.
        // Denial must come from the separately resolved read-only grant.
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: RealmId::parse("native-loop").expect("realm"),
            ingress: ResourceDomain {
                authority: principal("ingress"),
                namespace: "prompts".into(),
            },
            occurrence_scope: id("occurrence-7"),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .expect("immutable association claims")
}

const E1_MODEL: &str = "claude-sonnet-4-6";
const FINISHED: &str = "read completed in the same native run";
const MAX_RECEIPT_BYTES: usize = 128 * 1024;

#[derive(Default)]
struct Receiver {
    bodies: Mutex<Vec<Value>>,
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
        let receiver = Arc::new(Receiver::default());
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
    events.into_iter().fold(String::new(), |mut stream, event| {
        write!(stream, "data: {event}\n\n").expect("write event to string");
        stream
    })
}
fn start_message() -> Value {
    json!({"type":"message_start","message":{"id":"e1-response","type":"message","role":"assistant","model":E1_MODEL,"content":[],"stop_reason":null,"usage":{"input_tokens":1,"output_tokens":0}}})
}
fn sibling_response() -> String {
    let mut events = vec![start_message()];
    for (index, id, name) in [
        (0, DENIED_CALL, "delete_record"),
        (1, PERMITTED_CALL, "read_record"),
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
    match count {
        1 => (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            sibling_response(),
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

fn assert_wire_initial_prompt(body: &Value, saved: &Session) {
    assert_eq!(body["model"], E1_MODEL, "actual fixture request: {body}");
    let messages = body["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("actual first HTTP messages missing: {body}"));
    assert_eq!(
        messages.len(),
        4,
        "three exact context/prompt messages plus one typed model-refusal notice: {body}"
    );
    for (message, expected) in messages[..3].iter().zip([
        "deferred-context-s2",
        "turn-context-s2",
        "deferred-seed-s2\n\nturn-prompt-s2",
    ]) {
        assert_eq!(message["role"], "user", "actual fixture request: {body}");
        assert_eq!(
            wire_text(&message["content"]),
            expected,
            "actual fixture request: {body}"
        );
    }
    let notices: Vec<_> = saved
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::SystemNotice(notice) => Some(notice),
            _ => None,
        })
        .collect();
    let [notice] = notices.as_slice() else {
        panic!("exactly one stored model-refusal notice: {notices:?}; request: {body}");
    };
    assert_eq!(
        notice.kind,
        meerkat_core::SystemNoticeKind::Generic,
        "{body}"
    );
    let [
        meerkat_core::SystemNoticeBlock::RuntimeNotice {
            category,
            detail,
            payload,
        },
    ] = notice.blocks.as_slice()
    else {
        panic!("exact model operation refusal notice: {notice:?}; request: {body}");
    };
    assert_eq!(category, "operation_refused", "{body}");
    assert!(detail.is_none(), "{notice:?}; request: {body}");
    assert_eq!(
        payload.as_ref(),
        Some(&json!({"code": "operation_refused"})),
        "{body}"
    );
    assert_eq!(
        notice.body.as_deref(),
        Some(
            "The requested model operation is unavailable under current authorization. Continue with the authorized controller without provider-hosted capabilities, or choose another permitted action."
        ),
        "{body}"
    );
    assert_eq!(
        messages[3]["role"], "user",
        "actual fixture request: {body}"
    );
    assert_eq!(
        messages[3]["content"],
        json!(notice.model_projection_text()),
        "fourth message must be the complete canonical stored notice projection: {body}"
    );
}

fn assert_saved_transcript(saved: &Session, session_id: &SessionId) {
    assert_eq!(saved.id(), session_id);
    // Check the canonical conversation independently of the two store reads.
    // System instructions/notices keep their own typed lane.
    let conversation: Vec<_> = saved
        .messages()
        .iter()
        .filter(|message| !matches!(message, Message::System(_) | Message::SystemNotice(_)))
        .collect();
    let [
        Message::User(deferred),
        Message::User(turn_context),
        Message::User(prompt),
        Message::BlockAssistant(calls),
        Message::ToolResults { results, .. },
        Message::BlockAssistant(final_message),
    ] = conversation.as_slice()
    else {
        panic!("exact context/prompt/assistant/results/final transcript: {conversation:?}");
    };
    assert!(deferred.transcript_role.is_injected_context());
    assert_eq!(deferred.text_content(), "deferred-context-s2");
    assert!(turn_context.transcript_role.is_injected_context());
    assert_eq!(turn_context.text_content(), "turn-context-s2");
    assert!(prompt.transcript_role.is_conversational());
    assert_eq!(prompt.text_content(), "deferred-seed-s2\n\nturn-prompt-s2");
    assert_eq!(calls.stop_reason, Some(StopReason::ToolUse));
    let calls: Vec<_> = calls.tool_calls().collect();
    assert_eq!(calls.len(), 2);
    for (call, (id, name)) in calls.iter().zip([
        (DENIED_CALL, "delete_record"),
        (PERMITTED_CALL, "read_record"),
    ]) {
        assert_eq!(call.id, id);
        assert_eq!(call.name, name);
        assert_eq!(
            serde_json::from_str::<Value>(call.args.get()).unwrap(),
            json!({"record":"record-7"})
        );
    }
    assert_eq!(
        results.len(),
        2,
        "exactly one saved result per actual sibling call"
    );
    for id in [DENIED_CALL, PERMITTED_CALL] {
        assert_eq!(
            results
                .iter()
                .filter(|result| result.tool_use_id == id)
                .count(),
            1
        );
    }
    let refused = results
        .iter()
        .find(|result| result.tool_use_id == DENIED_CALL)
        .unwrap();
    assert!(refused.is_error);
    assert_eq!(
        serde_json::from_str::<Value>(&refused.text_content()).unwrap(),
        ToolError::AuthorizationRefused { refusal: denied() }.to_error_payload()
    );
    let permitted = results
        .iter()
        .find(|result| result.tool_use_id == PERMITTED_CALL)
        .unwrap();
    assert!(!permitted.is_error);
    assert_eq!(permitted.text_content(), "record-7 value");
    assert!(
        results
            .iter()
            .all(|result| result.settlement_failures.is_empty())
    );
    assert_eq!(final_message.stop_reason, Some(StopReason::EndTurn));
    assert_eq!(final_message.tool_calls().count(), 0);
    assert_eq!(final_message.text_blocks().collect::<String>(), FINISHED);
}

fn assert_wire_sibling_feedback(body: &Value) {
    let messages = body["messages"]
        .as_array()
        .expect("actual Anthropic request messages");
    let assistant = messages
        .iter()
        .find(|message| {
            message["role"] == "assistant"
                && message["content"].as_array().is_some_and(|blocks| {
                    blocks
                        .iter()
                        .filter(|block| block["type"] == "tool_use")
                        .count()
                        == 2
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
        (Some(DENIED_CALL), Some("delete_record"))
    );
    assert_eq!(
        (calls[1]["id"].as_str(), calls[1]["name"].as_str()),
        (Some(PERMITTED_CALL), Some("read_record"))
    );
    let results: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| message["content"].as_array())
        .flat_map(|blocks| blocks.iter())
        .filter(|block| block["type"] == "tool_result")
        .collect();
    assert_eq!(results.len(), 2, "exactly one result for each sibling");
    let refused = results
        .iter()
        .find(|result| result["tool_use_id"] == DENIED_CALL)
        .unwrap();
    let allowed = results
        .iter()
        .find(|result| result["tool_use_id"] == PERMITTED_CALL)
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

struct HttpRecordOwner {
    selected: ControllerModelSelection,
    endpoint: String,
}
impl OperationPolicyOwner for HttpRecordOwner {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        _: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        if association.candidate().controller_model.as_ref() != Some(&self.selected)
            || facts.selection() != &self.selected
            || facts.endpoint() != self.endpoint
            || facts.wire_model() != E1_MODEL
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
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let action = match &binding.facts().operation {
            AuthorizationOperation::Model(facts)
                if purpose == LocalPolicyPurpose::Controller
                    && self.selected.matches_model_facts(facts)
                    && facts.endpoint.as_ref() == self.endpoint
                    && facts.wire_model.as_ref() == E1_MODEL =>
            {
                "infer"
            }
            AuthorizationOperation::Tool(facts) if purpose == LocalPolicyPurpose::Operation => {
                let args: Value =
                    serde_json::from_str(facts.arguments.get()).map_err(|_| denied())?;
                if args != json!({"record":"record-7"}) {
                    return Err(denied().into());
                }
                match facts.name.as_str() {
                    "read_record" => "read",
                    "delete_record" => "delete",
                    _ => return Err(denied().into()),
                }
            }
            _ => return Err(denied().into()),
        };
        Ok(LocalPolicyAllowance {
            operation_values: values(action),
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now + 60_000,
        })
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

#[derive(Clone, PartialEq, Eq)]
struct InvocationPermission {
    runtime: LogicalRuntimeId,
    requester: PrincipalRef,
    ingress: PrincipalRef,
}
struct Fixture {
    setup: GovernedJsonlSetup,
    store: Arc<dyn RuntimeStore>,
    permissions: Arc<Mutex<Vec<InvocationPermission>>>,
    produced: Arc<Mutex<Vec<(Input, meerkat_core::ControllerModelClient)>>>,
}
async fn fixture(server: &Server) -> Fixture {
    let client = http_client(server);
    let selected = client.controller_model_selection().unwrap();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("s2-grants"),
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
            id("read"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .unwrap();
    let permissions: Arc<Mutex<Vec<InvocationPermission>>> = Arc::default();
    let produced: Arc<Mutex<Vec<(Input, meerkat_core::ControllerModelClient)>>> = Arc::default();
    let ingress: Arc<NativeIngressCheck> = Arc::new({
        let permissions = permissions.clone();
        let controller = controller.clone();
        let operation = operation.clone();
        let selected = selected.clone();
        move |runtime, _, current, claimed| {
            if current.requester() != &principal("requester")
                || current.ingress_actor() != &principal("ingress")
                || current.realm() != &RealmId::parse("native-loop").unwrap()
                || claimed
                    != &association(
                        runtime,
                        controller.clone(),
                        operation.clone(),
                        selected.clone(),
                    )
                || !permissions.lock().unwrap().contains(&InvocationPermission {
                    runtime: runtime.clone(),
                    requester: current.requester().clone(),
                    ingress: current.ingress_actor().clone(),
                })
            {
                return Err(denied().into());
            }
            Ok(())
        }
    });
    let connection = GovernedConnection::new(
        principal("requester"),
        None,
        principal("ingress"),
        RealmId::parse("native-loop").unwrap(),
        evidence("actual-jsonl-authentication"),
        Arc::new({
            let produced = produced.clone();
            move |runtime, input, pin| {
                produced.lock().unwrap().push((input.clone(), pin.clone()));
                Ok(association(
                    runtime,
                    controller.clone(),
                    operation.clone(),
                    pin.selection().clone(),
                ))
            }
        }),
    )
    .unwrap();
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let bundle = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        Arc::new(meerkat::MemoryStore::new()),
        store.clone(),
        Arc::new(meerkat::MemoryBlobStore::new()),
        NativeGrantWorkConfiguration {
            grants,
            ingress,
            invocation_owner: Arc::new(InvocationOwner),
            operation_owner: Arc::new(HttpRecordOwner {
                selected: selected.clone(),
                endpoint: format!("{}/v1/messages", server.base_url),
            }),
        },
    )
    .expect("construct governed persistence before credential publication");
    meerkat_auth_core::save_tokens_and_publish_lifecycle(
        meerkat_core::auth::ProviderAuthPersistence::new(
            Arc::new(meerkat_auth_core::EphemeralTokenStore::new()),
            Arc::new(meerkat_auth_core::InMemoryCoordinator::new()),
        ),
        bundle.runtime_adapter().generated_auth_lease_handle(),
        selected.credential().clone(),
        meerkat_core::auth::PersistedTokens::api_key("synthetic-e1-loopback-only"),
    )
    .await
    .unwrap();
    let mut config = Config::default();
    config.agent.model = E1_MODEL.into();
    config.agent.system_prompt = Some("commissioned-s2-system".into());
    config.skills.enabled = false;
    config.tools.schedule_enabled = false;
    // The selected override still names a real config-owned binding at RPC create.
    let auth_binding = selected.auth_binding().expect("configured fixture binding");
    let mut realm = meerkat_core::RealmConfigSection::default();
    realm.backend.insert(
        "e1-private-http".into(),
        meerkat_core::BackendProfileConfig {
            provider: "anthropic".into(),
            backend_kind: "anthropic_api".into(),
            base_url: Some(server.base_url.clone()),
            options: Value::Null,
            server: None,
        },
    );
    realm.auth.insert(
        "e1-private-authorizer".into(),
        meerkat_core::AuthProfileConfig {
            provider: "anthropic".into(),
            auth_method: "external_authorizer".into(),
            source: meerkat_core::CredentialSourceSpec::ExternalResolver {
                handle: "e1-private-loopback".into(),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    realm.binding.insert(
        auth_binding.binding.as_str().into(),
        meerkat_core::ProviderBindingConfig {
            backend_profile: "e1-private-http".into(),
            auth_profile: "e1-private-authorizer".into(),
            credential_account: None,
            default_model: Some(E1_MODEL.into()),
            policy: Default::default(),
            provider_default: false,
        },
    );
    config
        .realm
        .insert(auth_binding.realm.as_str().into(), realm);
    let target = meerkat_core::resolve_explicit_auth_binding_target(&config, auth_binding)
        .expect("fixture config resolves the actual selected binding");
    assert_eq!(&target.auth_binding, auth_binding);
    assert_eq!(&target.credential_identity, selected.credential());
    meerkat_providers::ProviderRuntimeCatalog::validate_binding_with_credential_identity(
        auth_binding,
        target.credential_identity.clone(),
        &target.backend,
        &target.auth_profile,
        &target.binding.policy,
    )
    .expect("fixture route is supported by the actual provider catalog");
    let tools = ["delete_record", "read_record"].into_iter().map(|name| ToolDef::new(
        name, "fixed record operation", json!({"type":"object","properties":{"record":{"type":"string"}},"required":["record"],"additionalProperties":false}),
    )).collect();
    Fixture {
        setup: GovernedJsonlSetup {
            config,
            persistence: bundle,
            client,
            connection,
            tools,
        },
        store,
        permissions,
        produced,
    }
}

async fn send(writer: &mut (impl tokio::io::AsyncWrite + Unpin), value: Value) {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    writer.write_all(&bytes).await.unwrap();
    writer.flush().await.unwrap();
}
async fn response(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: u64,
    effects: &Arc<Mutex<Vec<String>>>,
) -> Value {
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).await.unwrap() > 0,
            "server remains connected"
        );
        let value: Value = serde_json::from_str(&line).unwrap();
        if value["method"] == "tool/execute" {
            let name = value["params"]["name"].as_str().unwrap();
            effects.lock().unwrap().push(name.into());
            assert_eq!(
                name, "read_record",
                "denied delete never reaches physical callback receiver"
            );
            assert_eq!(value["params"]["arguments"], json!({"record":"record-7"}));
            send(writer, json!({"jsonrpc":"2.0","id":value["id"],"result":{"content":"record-7 value","is_error":false}})).await;
        } else if value["id"] == id {
            return value;
        }
    }
}
#[derive(Deserialize)]
struct AuditProjection {
    #[serde(default)]
    authorization_audit: Vec<StoredAuthorizationAuditObservation>,
}
fn audit(
    row: &meerkat_runtime::input_state::StoredInputState,
) -> Vec<StoredAuthorizationAuditObservation> {
    serde_json::from_value::<AuditProjection>(serde_json::to_value(row).unwrap())
        .unwrap()
        .authorization_audit
}

async fn cleanup_jsonl<E: std::fmt::Debug>(
    mut reader: impl tokio::io::AsyncBufRead + Unpin + Send + 'static,
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    mut server_task: tokio::task::JoinHandle<Result<(), E>>,
    http: &mut Server,
) -> Vec<String> {
    let mut failures = Vec::new();
    // A failed assertion must not leave the real model receiver held forever.
    http.receiver.finish.notify_one();
    let mut drain = tokio::spawn(async move {
        let mut line = String::new();
        while reader.read_line(&mut line).await.unwrap_or(0) > 0 {
            line.clear();
        }
    });
    match tokio::time::timeout(Duration::from_secs(2), writer.shutdown()).await {
        Ok(Ok(())) => {}
        other => failures.push(format!("JSONL writer shutdown: {other:?}")),
    }
    // Borrow the JoinHandle: timeout must not detach the server task.
    match tokio::time::timeout(Duration::from_secs(10), &mut server_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(other) => failures.push(format!("JSONL server completion: {other:?}")),
        Err(_) => {
            failures.push("JSONL server did not stop within the cleanup bound".into());
            server_task.abort();
            match tokio::time::timeout(Duration::from_secs(2), &mut server_task).await {
                Ok(Err(error)) if error.is_cancelled() => {}
                Ok(Ok(Ok(()))) => {}
                other => failures.push(format!("JSONL server abort/join: {other:?}")),
            }
        }
    }
    drain.abort();
    match tokio::time::timeout(Duration::from_secs(2), &mut drain).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) if error.is_cancelled() => {}
        other => failures.push(format!("JSONL drain abort/join: {other:?}")),
    }
    if let Some(mut task) = http.task.take() {
        task.abort();
        match tokio::time::timeout(Duration::from_secs(2), &mut task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.is_cancelled() => {}
            other => failures.push(format!("HTTP receiver abort/join: {other:?}")),
        }
    }
    failures
}

fn finish_scenario(
    result: std::thread::Result<Result<(), tokio::time::error::Elapsed>>,
    cleanup_failures: Vec<String>,
) {
    match result {
        Err(panic) => {
            if !cleanup_failures.is_empty() {
                eprintln!("secondary cleanup failures: {cleanup_failures:?}");
            }
            std::panic::resume_unwind(panic);
        }
        Ok(Err(timeout)) => {
            panic!("scenario exceeded its bound: {timeout}; cleanup: {cleanup_failures:?}")
        }
        Ok(Ok(())) => assert!(cleanup_failures.is_empty(), "{cleanup_failures:?}"),
    }
}

// Exercise the exported API independently of the private construct fixture.
// Both callers use the same trusted host setup and real callback/HTTP owners.
#[cfg(not(feature = "mcp"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_jsonl_public_entry_preserves_refusal_sibling_feedback_and_completed_native_run() {
    let mut http = Server::start().await;
    let Fixture {
        setup,
        store,
        permissions,
        produced,
    } = fixture(&http).await;
    let selected = setup.client.controller_model_selection().unwrap();
    let adapter = setup.persistence.runtime_adapter();
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (reader, writer) = tokio::io::split(server_io);
    let server_task = tokio::spawn(serve_governed_jsonl(BufReader::new(reader), writer, setup));
    let (reader, mut writer) = tokio::io::split(client_io);
    let mut reader = BufReader::new(reader);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let scenario = async {
        send(
            &mut writer,
            json!({"jsonrpc":"2.0","id":101,"method":"initialize"}),
        )
        .await;
        let initialized = response(&mut reader, &mut writer, 101, &effects).await;
        assert_eq!(
            initialized["result"]["methods"],
            json!([
                "initialize",
                "initialized",
                "cancel",
                "session/create",
                "turn/start"
            ])
        );
        send(&mut writer, json!({"jsonrpc":"2.0","id":102,"method":"session/create","params":{
            "prompt":"deferred-seed-s2", "injected_context":["deferred-context-s2"], "initial_turn":"deferred"
        }})).await;
        let created = response(&mut reader, &mut writer, 102, &effects).await;
        assert!(created.get("error").is_none(), "{created}");
        let sid = SessionId::parse(created["result"]["session_id"].as_str().unwrap()).unwrap();
        let rid = LogicalRuntimeId::for_session(&sid);
        assert!(
            store
                .load_input_states_strict(&rid)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(produced.lock().unwrap().is_empty());
        assert!(http.receiver.bodies.lock().unwrap().is_empty());
        assert!(effects.lock().unwrap().is_empty());
        permissions.lock().unwrap().push(InvocationPermission {
            runtime: rid.clone(),
            requester: principal("requester"),
            ingress: principal("ingress"),
        });
        send(&mut writer, json!({"jsonrpc":"2.0","id":103,"method":"turn/start","params":{
            "session_id":sid.to_string(), "prompt":"turn-prompt-s2", "injected_context":["turn-context-s2"]
        }})).await;
        let wait = response(&mut reader, &mut writer, 103, &effects);
        tokio::pin!(wait);
        tokio::select! {
            result = &mut wait => panic!("public entry completed before held model request2: {result}"),
            _ = http.receiver.second_request.notified() => {}
        }
        let (submitted, pin) = {
            let produced = produced.lock().unwrap();
            assert_eq!(produced.len(), 1, "exactly one real input association");
            produced[0].clone()
        };
        assert_eq!(pin.selection(), &selected);
        let Input::Prompt(prompt) = submitted else {
            panic!("real public prompt")
        };
        assert_eq!(
            prompt.content.text_content(),
            "deferred-seed-s2\n\nturn-prompt-s2"
        );
        assert_eq!(
            prompt
                .injected_context
                .iter()
                .map(|item| item.text_content())
                .collect::<Vec<_>>(),
            ["deferred-context-s2", "turn-context-s2"]
        );
        let live_rows = store.load_input_states_strict(&rid).await.unwrap();
        assert_eq!(live_rows.len(), 1);
        let input_id = live_rows[0].state.input_id.clone();
        let live_audit = audit(&live_rows[0]);
        assert!(!live_audit.is_empty());
        let run_id = live_audit[0].observation.run_id.clone().unwrap();
        let bodies = http.receiver.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 2);
        assert_wire_sibling_feedback(&bodies[1]);
        assert_eq!(*effects.lock().unwrap(), ["read_record"]);
        http.receiver.finish.notify_one();
        let result = wait.await;
        assert!(result.get("error").is_none(), "{result}");
        assert_eq!(result["result"]["text"], FINISHED);
        let row = store
            .load_input_state(&rid, &input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.seed.phase,
            meerkat_runtime::input_state::InputLifecycleState::Consumed
        );
        assert_eq!(row.seed.last_run_id.as_ref(), Some(&run_id));
        assert_eq!(
            row.seed.terminal_outcome,
            Some(meerkat_runtime::input_state::InputTerminalOutcome::Consumed)
        );
        assert!(row.state.persisted_input.is_none());
        let completion = adapter
            .input_terminal_completion(&sid, &input_id)
            .await
            .unwrap()
            .unwrap();
        let CompletionOutcome::Completed(completed) = completion else {
            panic!("completed public native run")
        };
        assert_eq!(completed.session_id, sid);
        assert_eq!(completed.text, FINISHED);
        assert!(completed.terminal_cause_kind.is_none());
        let final_audit = audit(&row);
        assert_eq!(
            final_audit.len(),
            11,
            "exact terminal model/tool operation audit"
        );
        assert!(final_audit.starts_with(&live_audit));
        for record in &final_audit {
            assert_eq!(record.observation.run_id.as_ref(), Some(&run_id));
            assert_eq!(record.contributors.len(), 1);
            assert_eq!(record.contributors[0].input_id, input_id);
            assert_eq!(record.contributors[0].requester, principal("requester"));
            assert_eq!(
                record.contributors[0].logical_executor,
                principal("executor")
            );
            assert!(record.contributors[0].represented_subject.is_none());
            assert!(matches!(&record.observation.execution_scope,
                OperationExecutionScope::RuntimeInput { owner_session_id, submitted_input_id, canonical_input_id, .. }
                if owner_session_id == &sid && submitted_input_id == &input_id && canonical_input_id == &input_id));
        }
        let denied: Vec<_> = final_audit
            .iter()
            .filter(|record| {
                matches!(&record.observation.observation,
            AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
            if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
                if call_id == DENIED_CALL && tool_name == "delete_record"))
            })
            .collect();
        assert_eq!(denied.len(), 1);
        let denied_id = &denied[0].observation.operation_id;
        assert_eq!(
            final_audit
                .iter()
                .filter(|record| &record.observation.operation_id == denied_id)
                .count(),
            1,
            "denied callback has only its refusal, no fabricated entry or outcome"
        );
        let prepared: Vec<_> = final_audit
            .iter()
            .filter(|record| {
                matches!(
                    record.observation.observation,
                    AuditObservation::Prepared { .. }
                )
            })
            .collect();
        assert_eq!(
            prepared.len(),
            3,
            "two actual model requests and one permitted read"
        );
        let mut model_count = 0;
        let mut read_count = 0;
        for record in prepared {
            let records: Vec<_> = final_audit
                .iter()
                .filter(|other| other.observation.operation_id == record.observation.operation_id)
                .collect();
            assert_eq!(records.len(), 3);
            assert!(matches!(
                records[1].observation.observation,
                AuditObservation::Entry
            ));
            match &record.observation.observation {
                AuditObservation::Prepared { target, .. }
                    if matches!(target.as_ref(), AuditTarget::Model(_)) =>
                {
                    model_count += 1;
                    assert!(matches!(
                        records[2].observation.observation,
                        AuditObservation::Outcome {
                            outcome: OperationObservedOutcome::HttpResponse { status: 200 }
                        }
                    ));
                }
                AuditObservation::Prepared { target, .. }
                    if matches!(target.as_ref(), AuditTarget::Tool {
                    call_id, tool_name, .. } if call_id == PERMITTED_CALL && tool_name == "read_record") =>
                {
                    read_count += 1;
                    assert!(
                        matches!(&records[2].observation.observation, AuditObservation::Outcome {
                        outcome: OperationObservedOutcome::ToolDispatchReturned { result_is_error: false,
                            terminal_error: None, asynchronous_operations }
                    } if asynchronous_operations.is_empty())
                    );
                }
                other => panic!("unexpected prepared target: {other:?}"),
            }
        }
        assert_eq!((model_count, read_count), (2, 1));
        let refused: Vec<_> = final_audit
            .iter()
            .filter(|record| {
                matches!(
                    record.observation.observation,
                    AuditObservation::Refused { .. }
                )
            })
            .collect();
        assert_eq!(
            refused.len(),
            2,
            "one denied tool plus the fixture's hosted-capability refusal"
        );
        assert_eq!(
            refused
                .iter()
                .filter(|record| matches!(&record.observation.observation,
            AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
            if matches!(target.as_ref(), AuditTarget::Model(_))))
                .count(),
            1
        );
        let document = store
            .load_committed_whole_blob_snapshot(&rid)
            .await
            .unwrap()
            .unwrap();
        let decoded = Session::decode_whole_blob_document(document.bytes()).unwrap();
        assert_eq!(
            decoded.row_sha256_token(),
            document.authority().blob_sha256()
        );
        let saved = decoded.into_session();
        assert_saved_transcript(&saved, &sid);
        assert_wire_initial_prompt(&bodies[0], &saved);
        assert_eq!(http.receiver.authorized_requests.load(Ordering::SeqCst), 2);
        assert_eq!(http.receiver.bodies.lock().unwrap().len(), 2);
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(25), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(reader, &mut writer, server_task, &mut http).await;
    drop(adapter);
    finish_scenario(result, cleanup_failures);
}

#[cfg(not(feature = "mcp"))]
async fn exercise_ordinary_constructor_refusal() {
    let mut http = Server::start().await;
    let Fixture {
        setup,
        store,
        produced,
        ..
    } = fixture(&http).await;
    let GovernedJsonlSetup {
        config,
        persistence,
        client,
        ..
    } = setup;
    let config_store: Arc<dyn ConfigStore> = Arc::new(MemoryConfigStore::new(
        config.clone(),
        meerkat_models::canonical(),
    ));
    let max_sessions = config.max_sessions();
    let runtime = Arc::new(SessionRuntime::new_with_config_store(
        AgentFactory::minimal(),
        config,
        config_store.clone(),
        max_sessions,
        persistence,
        NotificationSink::noop(),
    ));
    runtime.set_default_llm_client(Some(client));
    assert!(
        runtime
            .runtime_adapter()
            .has_native_work_authorization_host()
    );
    let (client_io, server_io) = tokio::io::duplex(65536);
    let (reader, writer) = tokio::io::split(server_io);
    let mut server = RpcServer::new(
        BufReader::new(reader),
        writer,
        runtime.clone(),
        config_store,
    )
    .unwrap();
    let server_task = tokio::spawn(async move { server.run().await });
    let (reader, mut writer) = tokio::io::split(client_io);
    let mut reader = BufReader::new(reader);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let scenario = async {
        send(
            &mut writer,
            json!({"jsonrpc":"2.0","id":201,"method":"initialize"}),
        )
        .await;
        let initialized = response(&mut reader, &mut writer, 201, &effects).await;
        assert!(
            initialized.get("error").is_none(),
            "ordinary connection remains usable"
        );
        for request_id in [202, 203] {
            send(
                &mut writer,
                json!({"jsonrpc":"2.0","id":request_id,"method":"session/create","params":{
                    "prompt":"deferred-seed-s2", "initial_turn":"deferred"
                }}),
            )
            .await;
            let refused = response(&mut reader, &mut writer, request_id, &effects).await;
            assert!(refused.get("result").is_none(), "{refused}");
            assert_eq!(
                refused["error"]["code"],
                meerkat_contracts::ErrorCode::InputNotReady.jsonrpc_code()
            );
            let detail: meerkat_contracts::wire::WireInputAdmissionErrorDetail =
                serde_json::from_value(refused["error"]["data"].clone()).unwrap();
            assert_eq!(
                detail,
                meerkat_contracts::wire::WireInputAdmissionErrorDetail::NotReady {
                    reason:
                        meerkat_contracts::wire::WireControllerReadinessFailure::UnsupportedScope {},
                }
            );
        }
        assert!(
            runtime
                .list_sessions(meerkat_core::service::SessionQuery::default())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .list_runtime_session_catalog_entries(meerkat_core::SessionFilter::default())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(produced.lock().unwrap().is_empty());
        assert!(http.receiver.bodies.lock().unwrap().is_empty());
        assert_eq!(http.receiver.authorized_requests.load(Ordering::SeqCst), 0);
        assert!(effects.lock().unwrap().is_empty());
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(15), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(reader, &mut writer, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
}

#[cfg(not(feature = "mcp"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_jsonl_refusal_retains_actor_seed_and_context_then_same_run_read_finishes() {
    let mut http = Server::start().await;
    let Fixture {
        setup,
        store,
        permissions,
        produced,
    } = fixture(&http).await;
    let exported_adapter = setup.persistence.runtime_adapter();
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (reader, writer) = tokio::io::split(server_io);
    let (mut server, runtime) = construct(BufReader::new(reader), writer, setup).unwrap();
    assert!(
        exported_adapter.shares_runtime_execution_owner_with(&runtime.runtime_adapter()),
        "the governed connection reuses its already exported execution owner"
    );
    let commissioned_tools = server.registered_tools();
    let callback_sender = server.callback_request_tx();
    let callback_id_counter = server.callback_id_counter();
    let server_task = tokio::spawn(async move { server.run().await });
    let (reader, mut writer) = tokio::io::split(client_io);
    let mut reader = BufReader::new(reader);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let scenario = async {
        assert_eq!(
            commissioned_tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            ["delete_record", "read_record"],
            "the server connection owns exactly the commissioned callback catalog"
        );
        assert!(
            runtime.default_callback_route().is_none(),
            "governed callbacks must not use the process-default route"
        );
        send(&mut writer, json!({"jsonrpc":"2.0","id":1,"method":"session/create","params":{
            "prompt":"deferred-seed-s2", "injected_context":["deferred-context-s2"], "initial_turn":"deferred"
        }})).await;
        let created = response(&mut reader, &mut writer, 1, &effects).await;
        assert!(created.get("error").is_none(), "{created}");
        let sid = SessionId::parse(created["result"]["session_id"].as_str().unwrap()).unwrap();
        let route = runtime
            .session_callback_route(&sid)
            .expect("deferred governed session retains its connection callback route");
        assert!(
            route.sender().same_channel(&callback_sender),
            "the session retains the actual server callback channel"
        );
        assert!(
            Arc::ptr_eq(&route.id_counter(), &callback_id_counter),
            "the session retains the actual server callback ID owner"
        );
        assert_eq!(
            serde_json::to_value(route.registry().snapshot()).unwrap(),
            serde_json::to_value(&commissioned_tools).unwrap(),
            "the session route retains the complete commissioned tool definitions"
        );
        let rid = LogicalRuntimeId::for_session(&sid);
        assert!(
            store
                .load_input_states_strict(&rid)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(http.receiver.bodies.lock().unwrap().is_empty());
        assert!(runtime.runtime_actor_witness_slot(&sid).witness().is_none());
        let turn = json!({"session_id":sid.to_string(), "prompt":"turn-prompt-s2", "injected_context":["turn-context-s2"]});
        send(
            &mut writer,
            json!({"jsonrpc":"2.0","id":2,"method":"turn/start","params":turn}),
        )
        .await;
        let refused = response(&mut reader, &mut writer, 2, &effects).await;
        assert!(
            refused.get("result").is_none(),
            "refusal cannot also carry success: {refused}"
        );
        assert_eq!(
            refused["error"]["code"], -32030,
            "canonical InputRefused code: {refused}"
        );
        let detail: meerkat_contracts::wire::WireInputAdmissionErrorDetail =
            serde_json::from_value(refused["error"]["data"].clone()).unwrap();
        assert!(matches!(
            detail,
            meerkat_contracts::wire::WireInputAdmissionErrorDetail::Refused {
                kind: meerkat_contracts::wire::WireInputRefusalKind::Denied
            }
        ));
        assert!(
            store
                .load_input_states_strict(&rid)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(http.receiver.bodies.lock().unwrap().is_empty());
        assert!(effects.lock().unwrap().is_empty());
        let actor_before = runtime
            .runtime_actor_witness_slot(&sid)
            .witness()
            .expect("real materialized actor survives refusal");
        assert!(actor_before.is_live());
        let (refused_input, refused_pin) = produced.lock().unwrap().last().unwrap().clone();
        let Input::Prompt(refused_prompt) = &refused_input else {
            panic!("actual prompt input")
        };
        assert_eq!(
            refused_prompt.content.text_content(),
            "deferred-seed-s2\n\nturn-prompt-s2"
        );
        assert_eq!(
            refused_prompt
                .injected_context
                .iter()
                .map(|x| x.text_content())
                .collect::<Vec<_>>(),
            ["deferred-context-s2", "turn-context-s2"]
        );
        permissions.lock().unwrap().push(InvocationPermission {
            runtime: rid.clone(),
            requester: principal("requester"),
            ingress: principal("ingress"),
        });
        send(
            &mut writer,
            json!({"jsonrpc":"2.0","id":3,"method":"turn/start","params":turn}),
        )
        .await;
        // Pump the real callback while the recording model holds request 2.
        let wait = response(&mut reader, &mut writer, 3, &effects);
        tokio::pin!(wait);
        tokio::select! {
            result = &mut wait => panic!("completion preceded the held second request: {result}"),
            _ = http.receiver.second_request.notified() => {}
        }
        let (accepted_input, accepted_pin) = produced.lock().unwrap().last().unwrap().clone();
        assert!(Arc::ptr_eq(refused_pin.client(), accepted_pin.client()));
        assert_eq!(
            runtime.runtime_actor_witness_slot(&sid).witness().unwrap(),
            actor_before
        );
        let Input::Prompt(accepted_prompt) = &accepted_input else {
            panic!("actual prompt input")
        };
        assert_eq!(accepted_prompt.content, refused_prompt.content);
        assert_eq!(
            accepted_prompt.injected_context,
            refused_prompt.injected_context
        );
        let rows = store.load_input_states_strict(&rid).await.unwrap();
        assert_eq!(rows.len(), 1);
        let input_id = rows[0].state.input_id.clone();
        let live = runtime
            .runtime_adapter()
            .input_state(&sid, &input_id)
            .await
            .unwrap()
            .unwrap();
        let Input::Prompt(stored_prompt) = live
            .state
            .persisted_input
            .as_ref()
            .expect("unfinished input retains its replay payload")
        else {
            panic!("retained unfinished prompt")
        };
        assert_eq!(stored_prompt.content, accepted_prompt.content);
        assert_eq!(
            stored_prompt.injected_context,
            accepted_prompt.injected_context
        );
        let live_audit = audit(&live);
        assert!(!live_audit.is_empty());
        let run_id = live_audit[0].observation.run_id.clone().unwrap();
        let bodies = http.receiver.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 2);
        assert_wire_sibling_feedback(&bodies[1]);
        assert_eq!(*effects.lock().unwrap(), ["read_record"]);
        http.receiver.finish.notify_one();
        let result = wait.await;
        assert!(result.get("error").is_none(), "{result}");
        assert_eq!(result["result"]["text"], FINISHED);
        let final_row = store
            .load_input_state(&rid, &input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            final_row.seed.phase,
            meerkat_runtime::input_state::InputLifecycleState::Consumed
        );
        assert_eq!(final_row.seed.last_run_id.as_ref(), Some(&run_id));
        assert_eq!(
            final_row.seed.terminal_outcome,
            Some(meerkat_runtime::input_state::InputTerminalOutcome::Consumed)
        );
        let completion = runtime
            .runtime_adapter()
            .input_terminal_completion(&sid, &input_id)
            .await
            .unwrap()
            .expect("actual finalized native completion");
        let CompletionOutcome::Completed(completed) = completion else {
            panic!("expected successful native completion, observed {completion:?}");
        };
        assert_eq!(completed.session_id, sid);
        assert_eq!(completed.text, FINISHED);
        assert!(
            completed.terminal_cause_kind.is_none(),
            "normal controller completion"
        );
        assert!(
            final_row.state.persisted_input.is_none(),
            "finalized input retires its replay payload; committed transcript retains history"
        );
        let final_audit = audit(&final_row);
        assert!(final_audit.starts_with(&live_audit));
        assert!(
            final_audit
                .iter()
                .all(|record| record.observation.run_id.as_ref() == Some(&run_id)
                    && record.contributors.len() == 1
                    && record.contributors[0].input_id == input_id)
        );
        for record in &final_audit {
            assert_eq!(record.contributors[0].requester, principal("requester"));
            assert_eq!(
                record.contributors[0].logical_executor,
                principal("executor")
            );
            assert!(record.contributors[0].represented_subject.is_none());
            assert!(matches!(&record.observation.execution_scope,
                OperationExecutionScope::RuntimeInput { owner_session_id, submitted_input_id, canonical_input_id, .. }
                if owner_session_id == &sid && submitted_input_id == &input_id && canonical_input_id == &input_id));
        }
        let refused: Vec<_> = final_audit
            .iter()
            .filter(|record| {
                matches!(&record.observation.observation,
            AuditObservation::Refused { target, reason: OperationRefusalKind::Denied }
            if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
                if call_id == DENIED_CALL && tool_name == "delete_record"))
            })
            .collect();
        assert_eq!(refused.len(), 1);
        let refused_id = &refused[0].observation.operation_id;
        assert!(
            !final_audit
                .iter()
                .any(|record| &record.observation.operation_id == refused_id
                    && matches!(
                        &record.observation.observation,
                        AuditObservation::Prepared { .. }
                            | AuditObservation::Entry
                            | AuditObservation::Outcome { .. }
                    )),
            "the denied delete has no operation entry or outcome"
        );
        let refused_models: Vec<_> = final_audit
            .iter()
            .filter(|record| {
                matches!(&record.observation.observation,
                AuditObservation::Refused { target, .. }
                if matches!(target.as_ref(), AuditTarget::Model(_)))
            })
            .collect();
        assert_eq!(
            refused_models.len(),
            1,
            "one requested model operation was refused"
        );
        let refused_model = &refused_models[0].observation;
        assert!(matches!(
            &refused_model.observation,
            AuditObservation::Refused {
                reason: OperationRefusalKind::Denied,
                ..
            }
        ));
        assert_ne!(&refused_model.operation_id, refused_id);
        assert!(
            !final_audit
                .iter()
                .any(
                    |record| record.observation.operation_id == refused_model.operation_id
                        && matches!(
                            &record.observation.observation,
                            AuditObservation::Prepared { .. }
                                | AuditObservation::Entry
                                | AuditObservation::Outcome { .. }
                        )
                ),
            "the refused model operation has no preparation, entry or outcome"
        );
        let reads: Vec<_> = final_audit
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                matches!(&record.observation.observation,
            AuditObservation::Prepared { target, .. }
            if matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
                if call_id == PERMITTED_CALL && tool_name == "read_record"))
            })
            .collect();
        assert_eq!(reads.len(), 1);
        let (read_prepared_at, read_prepared) = reads[0];
        let read_id = &read_prepared.observation.operation_id;
        assert_ne!(read_id, refused_id);
        let read_entry_at = final_audit
            .iter()
            .position(|record| {
                &record.observation.operation_id == read_id
                    && matches!(&record.observation.observation, AuditObservation::Entry)
            })
            .expect("actual read operation entry");
        let read_outcome_at = final_audit
            .iter()
            .position(|record| {
                &record.observation.operation_id == read_id
                    && matches!(&record.observation.observation, AuditObservation::Outcome {
                outcome: OperationObservedOutcome::ToolDispatchReturned {
                    result_is_error: false, terminal_error: None, asynchronous_operations,
                }
            } if asynchronous_operations.is_empty())
            })
            .expect("same read operation returned success");
        assert!(read_prepared_at < read_entry_at && read_entry_at < read_outcome_at);
        let models: Vec<_> = final_audit.iter().enumerate().filter(|(_, record)| matches!(&record.observation.observation,
            AuditObservation::Prepared { target, .. } if matches!(target.as_ref(), AuditTarget::Model(_)))).collect();
        assert_eq!(models.len(), 2);
        assert_ne!(
            models[0].1.observation.operation_id,
            models[1].1.observation.operation_id
        );
        for (prepared_at, model) in models {
            let operation_id = &model.observation.operation_id;
            let entry_at = final_audit
                .iter()
                .position(|record| {
                    &record.observation.operation_id == operation_id
                        && matches!(&record.observation.observation, AuditObservation::Entry)
                })
                .expect("actual model operation entry");
            let outcome_at = final_audit
                .iter()
                .position(|record| {
                    &record.observation.operation_id == operation_id
                        && matches!(
                            &record.observation.observation,
                            AuditObservation::Outcome {
                                outcome: OperationObservedOutcome::HttpResponse { status: 200 }
                            }
                        )
                })
                .expect("same model operation received actual HTTP 200");
            assert!(prepared_at < entry_at && entry_at < outcome_at);
        }
        let document = store
            .load_committed_whole_blob_snapshot(&rid)
            .await
            .unwrap()
            .unwrap();
        let decoded = Session::decode_whole_blob_document(document.bytes()).unwrap();
        assert_eq!(
            decoded.row_sha256_token(),
            document.authority().blob_sha256()
        );
        let saved = decoded.into_session();
        assert_saved_transcript(&saved, &sid);
        assert_wire_initial_prompt(&bodies[0], &saved);
        let service_history = runtime
            .persistent_service()
            .load_authoritative_session(&sid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(saved.messages()).unwrap(),
            serde_json::to_value(service_history.messages()).unwrap()
        );
        assert_eq!(http.receiver.authorized_requests.load(Ordering::SeqCst), 2);
        assert_eq!(http.receiver.bodies.lock().unwrap().len(), 2);
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(25), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(reader, &mut writer, server_task, &mut http).await;
    drop(exported_adapter);
    finish_scenario(result, cleanup_failures);
}

#[cfg(not(feature = "mcp"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_jsonl_rejects_ungoverned_bundle_and_unsupported_wire_before_setup() {
    let mut http = Server::start().await;
    let Fixture { mut setup, .. } = fixture(&http).await;
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    setup.persistence = meerkat::PersistenceBundle::new(
        Arc::new(meerkat::MemoryStore::new()),
        store.clone(),
        Arc::new(meerkat::MemoryBlobStore::new()),
    )
    .expect("construct an ordinary ungoverned persistence owner");
    let (read, write) = tokio::io::duplex(1024);
    assert!(matches!(
        construct(BufReader::new(read), write, setup),
        Err(GovernedJsonlError::Runtime(
            RuntimeDriverError::ControllerReadinessUnavailable {
                reason: meerkat_runtime::traits::ControllerReadinessFailure::UnsupportedScope,
            }
        ))
    ));
    assert!(http.receiver.bodies.lock().unwrap().is_empty());
    assert!(
        store
            .list_runtime_session_catalog_entries(meerkat_core::SessionFilter::default())
            .await
            .unwrap()
            .is_empty()
    );
    drop(store);

    let Fixture {
        setup, produced, ..
    } = fixture(&http).await;
    let (client_io, server_io) = tokio::io::duplex(65536);
    let (r, w) = tokio::io::split(server_io);
    let (mut server, runtime) = construct(BufReader::new(r), w, setup).unwrap();
    let server_task = tokio::spawn(async move { server.run().await });
    let (r, mut w) = tokio::io::split(client_io);
    let mut r = BufReader::new(r);
    let effects = Arc::default();
    let scenario = async {
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":19,"method":"initialize"}),
        )
        .await;
        let initialized = response(&mut r, &mut w, 19, &effects).await;
        assert_eq!(
            initialized["result"]["methods"],
            json!([
                "initialize",
                "initialized",
                "cancel",
                "session/create",
                "turn/start"
            ])
        );
        for (index, (method, params)) in [
            (
                "session/create",
                json!({"prompt":"x","initial_turn":"deferred","requester":"forged"}),
            ),
            (
                "session/create",
                json!({"prompt":"x","initial_turn":"deferred","enable_shell":true}),
            ),
            (
                "session/create",
                json!({"prompt":"x","initial_turn":"deferred","external_tools":[]}),
            ),
            ("session/create", json!({"prompt":"x"})),
            ("tools/register", json!({"tools":[]})),
            ("config/set", json!({"config":{}})),
            (
                "session/history",
                json!({"session_id":SessionId::new().to_string()}),
            ),
            (
                "turn/start",
                json!({"session_id":SessionId::new().to_string(),"prompt":"cold"}),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let id = index as u64 + 20;
            send(
                &mut w,
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .await;
            let result = response(&mut r, &mut w, id, &effects).await;
            assert!(
                result.get("error").is_some(),
                "unsupported request reached a handler: {result}"
            );
            assert!(
                runtime
                    .list_sessions(meerkat_core::service::SessionQuery::default())
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert!(produced.lock().unwrap().is_empty());
            assert!(http.receiver.bodies.lock().unwrap().is_empty());
            assert!(effects.lock().unwrap().is_empty());
        }
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(15), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
    exercise_ordinary_constructor_refusal().await;
}

#[cfg(feature = "mcp")]
#[tokio::test]
async fn governed_jsonl_mcp_feature_setup_is_unsupported_before_runtime_or_provider_work() {
    let mut http = Server::start().await;
    let Fixture {
        setup, produced, ..
    } = fixture(&http).await;
    let (r, w) = tokio::io::duplex(1024);
    let result = construct(BufReader::new(r), w, setup);
    assert!(matches!(
        result,
        Err(GovernedJsonlError::Runtime(
            meerkat_runtime::RuntimeDriverError::ControllerReadinessUnavailable {
                reason: meerkat_runtime::traits::ControllerReadinessFailure::UnsupportedScope
            }
        ))
    ));
    assert!(produced.lock().unwrap().is_empty());
    assert!(http.receiver.bodies.lock().unwrap().is_empty());
    http.reap().await;
}
