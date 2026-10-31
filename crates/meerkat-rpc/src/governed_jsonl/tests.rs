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
    ActionRef, AudienceRef, DomainRestriction, ExactRestriction, ExecutionRestrictions,
    ProcessorRef, ResourceDomain,
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
use meerkat_core::event::{AgentEvent, EventEnvelope};
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

/// The model's payload for the denial the fenced entry met before the
/// tool's code ran: the closed kind with `tool_invoked: false`.
fn denied_before_dispatch_payload() -> Value {
    let mut payload = ToolError::AuthorizationRefused { refusal: denied() }.to_error_payload();
    payload["data"]["tool_invoked"] = Value::Bool(false);
    payload
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
    bounds.resource_domains = DomainRestriction::exact([domain()]);
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
struct InvocationOwner {
    attachments: AttachmentLog,
}
impl AdmittedWorkPolicyOwner for InvocationOwner {
    /// The mandate owner verifies the attachment control's receipt is this
    /// connection's actual authentication for the exact session and bound
    /// target, and the control's exact facts.
    #[cfg(feature = "mcp")]
    fn authorize_mcp_attachment(
        &self,
        control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        _: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        let facts = control.facts();
        let realm = RealmId::parse("native-loop").unwrap();
        let receipt = control.evidence::<GovernedJsonlAttachmentReceipt>();
        let authenticated = receipt.is_some_and(|receipt| {
            receipt.authentication() == &evidence("actual-jsonl-authentication")
                && receipt.requester() == &facts.requester
                && receipt.ingress_actor() == &facts.actor
                && receipt.realm() == &realm
                && receipt.session_id() == control.session_id()
                && receipt.target() == &session_target(control.session_id())
        }) && facts.requester == principal("requester")
            && facts.actor == principal("ingress")
            && facts.realm == realm;
        self.attachments.lock().unwrap().push(AttachmentConsult {
            owner: "mandate",
            server: control.server().name.clone(),
            allowed: authenticated,
        });
        if !authenticated {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }

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

/// The scoped destination the trusted setup binds for `session`: its context
/// is the session itself.
fn session_target(session: &SessionId) -> NativeWorkTarget {
    NativeWorkTarget {
        logical_owner: principal("native-owner"),
        logical_runtime: id(&LogicalRuntimeId::for_session(session).to_string()),
        context: id(&session.to_string()),
        context_generation: 1,
        audience: AudienceRef::Principal {
            principal: principal("requester"),
        },
    }
}

/// Targets the trusted setup bound, by runtime.
type BoundTargets = Arc<Mutex<HashMap<String, NativeWorkTarget>>>;

/// The native destination fixture inputs of `runtime` name: the bound
/// target when the setup bound one, otherwise an unscoped fixture target.
fn input_target(bound: &BoundTargets, runtime: &LogicalRuntimeId) -> NativeWorkTarget {
    bound
        .lock()
        .unwrap()
        .get(&runtime.to_string())
        .cloned()
        .unwrap_or_else(|| native_target(runtime))
}

/// The trusted setup's binder: it binds the target `produce` yields (none
/// refuses) on the session's current registration and records it for the
/// fixture inputs of that runtime.
struct FixtureBinder {
    bound: BoundTargets,
    produce: Arc<dyn Fn(&SessionId) -> Option<NativeWorkTarget> + Send + Sync>,
}

#[async_trait]
impl GovernedSessionBinder for FixtureBinder {
    async fn bind_session(
        &self,
        machine: &MeerkatMachine,
        session_id: &SessionId,
    ) -> Result<NativeWorkTarget, RuntimeDriverError> {
        let target = (self.produce)(session_id).ok_or_else(unsupported)?;
        let registration = machine
            .current_session_registration_witness(session_id)
            .await
            .ok_or_else(unsupported)?;
        machine.bind_scoped_context(&registration, &target).await?;
        self.bound
            .lock()
            .unwrap()
            .insert(target.logical_runtime.as_str().to_owned(), target.clone());
        Ok(target)
    }
}

/// The native destination every unscoped fixture input of `runtime` names.
fn native_target(runtime: &LogicalRuntimeId) -> NativeWorkTarget {
    NativeWorkTarget {
        logical_owner: principal("native-owner"),
        logical_runtime: id(&runtime.to_string()),
        context: id("native-context"),
        context_generation: 1,
        audience: AudienceRef::Principal {
            principal: principal("requester"),
        },
    }
}

fn association(
    target: NativeWorkTarget,
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
        target,
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
    /// Answer every request with the final text, never a tool call.
    direct_answer: std::sync::atomic::AtomicBool,
    /// Answer the next request with one call of this tool (taken once),
    /// before any other answer.
    tool_call: Mutex<Option<String>>,
    /// Hold the next request (taken once): notify `second_request`, then
    /// answer with the final text only once `finish` is notified.
    hold_next: std::sync::atomic::AtomicBool,
    second_request: Notify,
    finish: Notify,
    authorized_requests: AtomicUsize,
    /// The name of the thread each model request's authorization ran on:
    /// the thread of the turn that sent the request.
    authorization_threads: Mutex<Vec<Option<String>>>,
    /// Answers taken in order, before any other answer, one per request.
    #[cfg(feature = "mcp")]
    script: Mutex<std::collections::VecDeque<ScriptedAnswer>>,
}

/// One scripted model answer: a call of `tool` as `call_id`. A held answer
/// first notifies `second_request`, and is given only once `finish` is
/// notified.
#[cfg(feature = "mcp")]
struct ScriptedAnswer {
    call_id: &'static str,
    tool: &'static str,
    held: bool,
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
    /// Abort the loopback receiver and join it within its bound, recording
    /// a failure to stop in `failures`.
    async fn join_receiver(&mut self, failures: &mut Vec<String>) {
        if let Some(mut task) = self.task.take() {
            task.abort();
            match tokio::time::timeout(Duration::from_secs(2), &mut task).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) if error.is_cancelled() => {}
                other => failures.push(format!("HTTP receiver abort/join: {other:?}")),
            }
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
const SINGLE_CALL: &str = "toolu_single";
fn single_tool_call_response(name: &str) -> String {
    tool_call_response(SINGLE_CALL, name)
}
fn tool_call_response(call_id: &str, name: &str) -> String {
    sse(vec![
        start_message(),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":call_id,"name":name,"input":{}}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","usage":{"output_tokens":2},"delta":{"stop_reason":"tool_use"}}),
        json!({"type":"message_stop"}),
    ])
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
    #[cfg(feature = "mcp")]
    {
        let scripted = receiver.script.lock().unwrap().pop_front();
        if let Some(answer) = scripted {
            if answer.held {
                receiver.second_request.notify_one();
                receiver.finish.notified().await;
            }
            return (
                StatusCode::OK,
                [("content-type", "text/event-stream")],
                tool_call_response(answer.call_id, answer.tool),
            );
        }
    }
    if receiver.hold_next.swap(false, Ordering::SeqCst) {
        receiver.second_request.notify_one();
        receiver.finish.notified().await;
        return (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            final_response(),
        );
    }
    let tool_call = receiver.tool_call.lock().unwrap().take();
    if let Some(name) = tool_call {
        return (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            single_tool_call_response(&name),
        );
    }
    if receiver.direct_answer.load(Ordering::SeqCst) {
        return (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            final_response(),
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
        self.0
            .authorization_threads
            .lock()
            .unwrap()
            .push(std::thread::current().name().map(str::to_owned));
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
    http_client_in(server, "native-loop")
}
/// The E1 HTTP client whose selection names a binding of `realm`.
fn http_client_in(server: &Server, realm: &str) -> Arc<dyn LlmClient> {
    http_client_with_binding(server, configured_binding_in(realm))
}

/// A fresh configured auth binding of `realm` for the E1 HTTP client.
fn configured_binding_in(realm: &str) -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse(realm).unwrap(),
        binding: BindingId::parse(format!("e1-{}", SessionId::new())).unwrap(),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

/// The E1 HTTP client whose selection names exactly `binding`. A realm's
/// configured binding is durable: every host process of the realm resolves
/// the same configured binding, each with its own client.
fn http_client_with_binding(server: &Server, binding: AuthBindingRef) -> Arc<dyn LlmClient> {
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
            content,
        },
    ] = notice.blocks.as_slice()
    else {
        panic!("exact model operation refusal notice: {notice:?}; request: {body}");
    };
    assert_eq!(category, "operation_refused", "{body}");
    assert!(detail.is_none(), "{notice:?}; request: {body}");
    assert!(content.is_empty(), "{notice:?}; request: {body}");
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
        denied_before_dispatch_payload()
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
    assert_eq!(diagnostic, denied_before_dispatch_payload());
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

/// One native owner decision on a governed MCP attachment control.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AttachmentConsult {
    owner: &'static str,
    server: String,
    allowed: bool,
}
type AttachmentLog = Arc<Mutex<Vec<AttachmentConsult>>>;

const SENTINEL_SERVER: &str = "records-sentinel";

struct HttpRecordOwner {
    selected: ControllerModelSelection,
    endpoint: String,
    attachments: AttachmentLog,
}
impl OperationPolicyOwner for HttpRecordOwner {
    /// The operation owner allows exactly the sentinel server, hydrated and
    /// published to the session's bound destination.
    fn authorize_mcp_attachment(
        &self,
        control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding,
        now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let destination = meerkat_runtime::meerkat_machine::scoped_destination_target(
            &session_target(control.session_id()),
        );
        let same = |target: &meerkat_core::OwnerQualifiedTarget| {
            target.authority == destination.authority
                && target.namespace == destination.namespace
                && target.id == destination.id
        };
        let operation = match &binding.facts().operation {
            AuthorizationOperation::Source(source)
                if source.usage == meerkat_core::authorization::SourceAuthorizationUse::Hydrate =>
            {
                true
            }
            AuthorizationOperation::Publication(publication) => matches!(
                &publication.recipient,
                meerkat_core::authorization::PublicationRecipient::Destination(target)
                    if same(target)
            ),
            _ => false,
        };
        let allowed = operation
            && control.server().name == SENTINEL_SERVER
            && same(&control.facts().destination);
        self.attachments.lock().unwrap().push(AttachmentConsult {
            owner: "operation",
            server: control.server().name.clone(),
            allowed,
        });
        if !allowed {
            return Err(denied().into());
        }
        Ok(LocalPolicyAllowance {
            operation_values: values("attach"),
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now + 60_000,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }

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
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
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
    #[cfg_attr(not(feature = "mcp"), allow(dead_code))]
    attachments: AttachmentLog,
    store: Arc<dyn RuntimeStore>,
    permissions: Arc<Mutex<Vec<InvocationPermission>>>,
    produced: Arc<Mutex<Vec<(Input, meerkat_core::ControllerModelClient)>>>,
}
async fn fixture(server: &Server) -> Fixture {
    fixture_with(server, "actual-jsonl-authentication", false).await
}
/// `authentication` is the evidence the connection retains; `mcp` commissions
/// live `mcp/add` with the host-bound destination owner.
async fn fixture_with(server: &Server, authentication: &str, mcp: bool) -> Fixture {
    let attachments: AttachmentLog = Arc::default();
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
    let bound: BoundTargets = Arc::default();
    let ingress: Arc<NativeIngressCheck> = Arc::new({
        let bound = bound.clone();
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
                        input_target(&bound, runtime),
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
        evidence(authentication),
        Arc::new({
            let produced = produced.clone();
            let bound = bound.clone();
            move |runtime, input, pin| {
                produced.lock().unwrap().push((input.clone(), pin.clone()));
                Ok(association(
                    input_target(&bound, runtime),
                    controller.clone(),
                    operation.clone(),
                    pin.selection().clone(),
                ))
            }
        }),
    )
    .unwrap();
    // The trusted setup binds each created session's destination; the
    // connection binds its inputs with its own association.
    let session_binder = mcp.then(|| {
        Arc::new(FixtureBinder {
            bound: bound.clone(),
            produce: Arc::new(|session: &SessionId| Some(session_target(session))),
        }) as Arc<dyn GovernedSessionBinder>
    });
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let bundle = meerkat::PersistenceBundle::new_with_local_grant_authorization(
        Arc::new(meerkat::MemoryStore::new()),
        store.clone(),
        Arc::new(meerkat::MemoryBlobStore::new()),
        NativeGrantWorkConfiguration {
            retained_resume: None,
            require_scoped_destination: mcp,
            grants,
            ingress,
            invocation_owner: Arc::new(InvocationOwner {
                attachments: attachments.clone(),
            }),
            operation_owner: Arc::new(HttpRecordOwner {
                attachments: attachments.clone(),
                selected: selected.clone(),
                endpoint: format!("{}/v1/messages", server.base_url),
            }),
            configured_mcp_attachment: None,
        },
    )
    .expect("construct governed persistence before credential publication");
    let (config, tools) = governed_client_config(server, &selected, &bundle).await;
    Fixture {
        attachments,
        setup: GovernedJsonlSetup {
            config,
            persistence: bundle,
            client,
            connection,
            tools,
            mcp_attachment_identity_key_custody: None,
            session_binder,
            connector_authority: None,
            operation_review: None,
            consent: None,
            host_confinement: meerkat::HostConfinement::trusted_host(),
        },
        store,
        permissions,
        produced,
    }
}

/// Publish the E1 credential on `bundle`'s machine and build the config whose
/// realm binding routes `selected` to the private HTTP receiver, plus the
/// fixed record tools.
async fn governed_client_config(
    server: &Server,
    selected: &ControllerModelSelection,
    bundle: &meerkat::PersistenceBundle,
) -> (Config, Vec<ToolDef>) {
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
    (config, tools)
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
    response_with_events(reader, writer, id, effects, None).await
}

type SessionNotifications = Arc<Mutex<Vec<(SessionId, EventEnvelope<AgentEvent>)>>>;

async fn response_with_events(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: u64,
    effects: &Arc<Mutex<Vec<String>>>,
    notifications: Option<&SessionNotifications>,
) -> Value {
    let mut matching_response = None;
    let mut terminal_seen = false;
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
        } else if value["method"] == "session/event"
            && let Some(notifications) = notifications
        {
            let session_id = SessionId::parse(
                value["params"]["session_id"]
                    .as_str()
                    .expect("actual notification session"),
            )
            .unwrap();
            let event: EventEnvelope<AgentEvent> =
                serde_json::from_value(value["params"]["event"].clone())
                    .expect("actual canonical native event envelope");
            terminal_seen |= matches!(
                &event.payload,
                AgentEvent::RunCompleted { .. } | AgentEvent::RunFailed { .. }
            );
            notifications.lock().unwrap().push((session_id, event));
            if terminal_seen && let Some(result) = matching_response.take() {
                return result;
            }
        } else if value["id"] == id {
            if notifications.is_none() || terminal_seen || value.get("error").is_some() {
                return value;
            }
            // Response and notification queues are selected independently.
            // Keep the result while waiting for its actual terminal event.
            matching_response = Some(value);
        }
    }
}

#[cfg(not(feature = "mcp"))]
fn assert_public_notifications(
    notifications: &SessionNotifications,
    session_id: &SessionId,
    run_id: &meerkat_core::lifecycle::RunId,
) {
    let events = notifications.lock().unwrap();
    for (route, event) in events.iter() {
        assert_eq!(route, session_id);
        assert_eq!(event.source.session_id(), Some(session_id));
    }
    assert!(events.windows(2).all(|pair| pair[0].1.seq < pair[1].1.seq));
    let starts: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(position, (_, event))| match &event.payload {
            AgentEvent::RunStarted {
                session_id,
                identity,
                ..
            } => Some((position, session_id, identity)),
            _ => None,
        })
        .collect();
    let ends: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(position, (_, event))| match &event.payload {
            AgentEvent::RunCompleted {
                session_id,
                identity,
                result,
                terminal_cause_kind,
                ..
            } => Some((position, session_id, identity, result, terminal_cause_kind)),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 1, "one real native run start");
    assert_eq!(ends.len(), 1, "one real native run completion");
    assert_eq!(starts[0].1, session_id);
    assert_eq!(ends[0].1, session_id);
    assert_eq!(starts[0].2.run_id.as_ref(), Some(run_id));
    assert_eq!(ends[0].2.run_id.as_ref(), Some(run_id));
    assert_eq!(ends[0].3, FINISHED);
    assert!(ends[0].4.is_none());
    assert!(
        !events
            .iter()
            .any(|(_, event)| matches!(&event.payload, AgentEvent::RunFailed { .. }))
    );
    let completions: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(position, (_, event))| match &event.payload {
            AgentEvent::ToolExecutionCompleted {
                id, name, is_error, ..
            } => Some((position, id.as_str(), name.as_str(), *is_error)),
            _ => None,
        })
        .collect();
    let results: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(position, (_, event))| match &event.payload {
            AgentEvent::ToolResultReceived {
                id,
                name,
                content,
                is_error,
            } => Some((
                position,
                id.as_str(),
                name.as_str(),
                content.as_slice(),
                *is_error,
            )),
            _ => None,
        })
        .collect();
    let expected = [
        (DENIED_CALL, "delete_record", true),
        (PERMITTED_CALL, "read_record", false),
    ];
    // The execution facts carry no body; the results carry it.
    assert_eq!(
        completions.len(),
        2,
        "one completion per actual sibling call"
    );
    for ((_, id, name, is_error), (expected_id, expected_name, expected_error)) in
        completions.iter().zip(expected)
    {
        assert_eq!(*id, expected_id);
        assert_eq!(*name, expected_name);
        assert_eq!(*is_error, expected_error);
    }
    assert_eq!(results.len(), 2, "one result per actual sibling call");
    for ((_, id, name, content, is_error), (expected_id, expected_name, expected_error)) in
        results.iter().zip(expected)
    {
        assert_eq!(*id, expected_id);
        assert_eq!(*name, expected_name);
        assert_eq!(*is_error, expected_error);
        let text = meerkat_core::types::text_content(content);
        if expected_error {
            assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap(),
                denied_before_dispatch_payload()
            );
        } else {
            assert_eq!(text, "record-7 value");
        }
    }
    let ordered = [
        starts[0].0,
        completions[0].0,
        results[0].0,
        completions[1].0,
        results[1].0,
        ends[0].0,
    ];
    assert!(
        ordered.windows(2).all(|pair| pair[0] < pair[1]),
        "canonical sibling feedback stays ordered inside the same native run"
    );
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
    http.join_receiver(&mut failures).await;
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

/// The name of the host runtime's threads in
/// [`a_governed_input_runs_its_staged_session_on_the_runtime_that_admits_it`].
const GOVERNED_HOST_WORKER_THREAD: &str = "governed-host-worker";

/// A governed input to a staged session materializes the session's actor on
/// the runtime of the caller that admits the input, so the session's turn
/// runs on that runtime's workers. The admission itself runs on Core's
/// process-wide machine cleanup runtime; the actor used to be materialized
/// there too, which put every turn of the session on that runtime's two
/// workers. The model request's authorization, which the turn awaits,
/// records the thread the turn runs on.
#[test]
fn a_governed_input_runs_its_staged_session_on_the_runtime_that_admits_it() {
    let host = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name(GOVERNED_HOST_WORKER_THREAD)
        .thread_stack_size(meerkat_runtime::host_stack::HOST_WORKER_STACK_BUDGET)
        .enable_all()
        .build()
        .expect("host runtime");
    let threads = host.block_on(async {
        let mut http = Server::start().await;
        http.receiver.direct_answer.store(true, Ordering::SeqCst);
        let Fixture {
            setup, permissions, ..
        } = fixture_with(&http, "actual-jsonl-authentication", cfg!(feature = "mcp")).await;
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (reader, writer) = tokio::io::split(server_io);
        let (mut server, runtime) = construct(BufReader::new(reader), writer, setup).unwrap();
        let server_task = tokio::spawn(async move { server.run().await });
        let (reader, mut writer) = tokio::io::split(client_io);
        let mut reader = BufReader::new(reader);
        let effects: Arc<Mutex<Vec<String>>> = Arc::default();
        let scenario = async {
            send(
                &mut writer,
                json!({"jsonrpc":"2.0","id":1,"method":"session/create","params":{
                    "prompt":"deferred-seed-host-runtime", "initial_turn":"deferred"
                }}),
            )
            .await;
            let created = response(&mut reader, &mut writer, 1, &effects).await;
            assert!(created.get("error").is_none(), "{created}");
            let sid = SessionId::parse(created["result"]["session_id"].as_str().unwrap()).unwrap();
            assert!(
                runtime.runtime_actor_witness_slot(&sid).witness().is_none(),
                "the deferred session has no actor before its first governed input"
            );
            permissions.lock().unwrap().push(InvocationPermission {
                runtime: LogicalRuntimeId::for_session(&sid),
                requester: principal("requester"),
                ingress: principal("ingress"),
            });
            send(
                &mut writer,
                json!({"jsonrpc":"2.0","id":2,"method":"turn/start","params":{
                    "session_id":sid.to_string(), "prompt":"turn-prompt-host-runtime"
                }}),
            )
            .await;
            let finished = response(&mut reader, &mut writer, 2, &effects).await;
            assert!(finished.get("error").is_none(), "{finished}");
            assert_eq!(finished["result"]["text"], FINISHED);
        };
        let result =
            std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(25), scenario))
                .catch_unwind()
                .await;
        let cleanup_failures = cleanup_jsonl(reader, &mut writer, server_task, &mut http).await;
        finish_scenario(result, cleanup_failures);
        http.receiver.authorization_threads.lock().unwrap().clone()
    });
    host.shutdown_timeout(Duration::from_secs(10));
    assert_eq!(
        threads.len(),
        1,
        "the turn authorized one model request: {threads:?}"
    );
    let thread = threads[0].as_deref();
    assert_ne!(
        thread,
        Some("meerkat-machine-cleanup"),
        "the staged session's turn ran on Core's machine cleanup runtime instead of the \
         runtime that admitted its governed input"
    );
    assert_eq!(
        thread,
        Some(GOVERNED_HOST_WORKER_THREAD),
        "the staged session's turn runs on a worker of the runtime that admitted its input"
    );
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
        ..
    } = fixture(&http).await;
    let selected = setup.client.controller_model_selection().unwrap();
    let adapter = setup.persistence.runtime_adapter();
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (reader, writer) = tokio::io::split(server_io);
    let server_task = tokio::spawn(serve_governed_jsonl(BufReader::new(reader), writer, setup));
    let (reader, mut writer) = tokio::io::split(client_io);
    let mut reader = BufReader::new(reader);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let notifications: SessionNotifications = Arc::default();
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
        let wait = response_with_events(
            &mut reader,
            &mut writer,
            103,
            &effects,
            Some(&notifications),
        );
        tokio::pin!(wait);
        tokio::select! {
            result = &mut wait => panic!("public entry completed before held model request2: {result}"),
            () = http.receiver.second_request.notified() => {}
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
        let live = adapter.input_state(&sid, &input_id).await.unwrap().unwrap();
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
        assert_public_notifications(&notifications, &sid, &run_id);
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
            AuditObservation::Refused { target, reason: OperationRefusalKind::Denied, .. }
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
                AuditObservation::Entry { .. }
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
        let model_prepared: Vec<_> = final_audit
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                matches!(&record.observation.observation,
                AuditObservation::Prepared { target, .. }
                if matches!(target.as_ref(), AuditTarget::Model(_)))
            })
            .collect();
        let first_model_id = &model_prepared[0].1.observation.operation_id;
        let first_model_outcome_at = final_audit
            .iter()
            .position(|record| {
                &record.observation.operation_id == first_model_id
                    && matches!(
                        record.observation.observation,
                        AuditObservation::Outcome { .. }
                    )
            })
            .unwrap();
        let denied_at = final_audit
            .iter()
            .position(|record| &record.observation.operation_id == denied_id)
            .unwrap();
        let read_prepared_at = final_audit
            .iter()
            .position(|record| {
                matches!(&record.observation.observation,
                AuditObservation::Prepared { target, .. }
                if matches!(target.as_ref(), AuditTarget::Tool { call_id, .. }
                    if call_id == PERMITTED_CALL))
            })
            .unwrap();
        let read_id = &final_audit[read_prepared_at].observation.operation_id;
        let read_outcome_at = final_audit
            .iter()
            .position(|record| {
                &record.observation.operation_id == read_id
                    && matches!(
                        record.observation.observation,
                        AuditObservation::Outcome { .. }
                    )
            })
            .unwrap();
        // Sibling dispatches may overlap; both finish between the model calls.
        assert!(first_model_outcome_at < denied_at);
        assert!(first_model_outcome_at < read_prepared_at);
        assert!(denied_at < model_prepared[1].0);
        assert!(read_outcome_at < model_prepared[1].0);
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
            AuditObservation::Refused { target, reason: OperationRefusalKind::Denied, .. }
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
    let runtime = Arc::new(
        SessionRuntime::new_with_config_store(
            AgentFactory::minimal(),
            config,
            config_store.clone(),
            max_sessions,
            persistence,
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime"),
    );
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
        ..
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
            () = http.receiver.second_request.notified() => {}
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
            AuditObservation::Refused { target, reason: OperationRefusalKind::Denied, .. }
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
                            | AuditObservation::Entry { .. }
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
                                | AuditObservation::Entry { .. }
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
                    && matches!(
                        &record.observation.observation,
                        AuditObservation::Entry { .. }
                    )
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
                        && matches!(
                            &record.observation.observation,
                            AuditObservation::Entry { .. }
                        )
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

/// One sentinel MCP server whose only effect, if it were ever spawned, is to
/// create `marker`.
#[cfg(feature = "mcp")]
fn sentinel_server(marker: &std::path::Path) -> meerkat_core::McpServerConfig {
    meerkat_core::McpServerConfig::stdio(
        SENTINEL_SERVER,
        "/bin/sh",
        vec!["-c".into(), format!("touch '{}'", marker.display())],
        HashMap::new(),
    )
}

#[cfg(feature = "mcp")]
fn mcp_add(id: u64, session: &SessionId, server: &meerkat_core::McpServerConfig) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"mcp/add","params":{
        "session_id": session.to_string(),
        "server_config": serde_json::to_value(server).unwrap(),
    }})
}

#[cfg(feature = "mcp")]
fn assert_attachment_refused(
    result: &Value,
    refusal: meerkat_core::mcp_attachment::McpAttachmentRefusal,
) {
    assert!(result.get("result").is_none(), "{result}");
    assert_eq!(result["error"]["code"], error::INVALID_REQUEST, "{result}");
    assert_eq!(
        result["error"]["message"],
        meerkat::surface::McpLiveAddError::Refused(refusal).to_string(),
        "{result}"
    );
}

#[cfg(feature = "mcp")]
fn assert_unsupported_scope(result: &Value) {
    let expected = crate::session_runtime::runtime_driver_error_to_rpc(unsupported());
    assert!(result.get("result").is_none(), "{result}");
    assert_eq!(result["error"]["code"], expected.code, "{result}");
    assert_eq!(result["error"]["message"], expected.message, "{result}");
}

/// Create a deferred governed session and complete its first native run, so
/// it has a current registration with a live executor attachment.
#[cfg(feature = "mcp")]
async fn governed_session_after_turn(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    effects: &Arc<Mutex<Vec<String>>>,
    permissions: &Arc<Mutex<Vec<InvocationPermission>>>,
    http: &Server,
) -> SessionId {
    send(
        writer,
        json!({"jsonrpc":"2.0","id":302,"method":"session/create","params":{
            "prompt":"deferred-seed-s4", "initial_turn":"deferred"
        }}),
    )
    .await;
    let created = response(reader, writer, 302, effects).await;
    assert!(created.get("error").is_none(), "{created}");
    let sid = SessionId::parse(created["result"]["session_id"].as_str().unwrap()).unwrap();
    permissions.lock().unwrap().push(InvocationPermission {
        runtime: LogicalRuntimeId::for_session(&sid),
        requester: principal("requester"),
        ingress: principal("ingress"),
    });
    send(
        writer,
        json!({"jsonrpc":"2.0","id":303,"method":"turn/start","params":{
            "session_id":sid.to_string(), "prompt":"turn-prompt-s4"
        }}),
    )
    .await;
    let wait = response(reader, writer, 303, effects);
    tokio::pin!(wait);
    tokio::select! {
        result = &mut wait => panic!("turn completed before the held model request: {result}"),
        () = http.receiver.second_request.notified() => {}
    }
    http.receiver.finish.notify_one();
    let finished = wait.await;
    assert!(finished.get("error").is_none(), "{finished}");
    assert_eq!(finished["result"]["text"], FINISHED);
    sid
}

/// Apply the session's boundary until no earlier connection is pending, so
/// a later boundary's count covers only what was staged after it.
#[cfg(feature = "mcp")]
async fn settle_mcp(runtime: &Arc<SessionRuntime>, sid: &SessionId) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while runtime.mcp_apply_staged_for_test(sid).await.unwrap() > 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the earlier connection settled");
}

/// Wait until a staged sentinel's spawn created `marker`.
#[cfg(feature = "mcp")]
async fn wait_for_marker(marker: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the applied server was started");
}

/// Retire the session's bound destination, as its registration owner does
/// when the destination is withdrawn.
#[cfg(feature = "mcp")]
async fn retire_destination(runtime: &Arc<SessionRuntime>, sid: &SessionId) {
    let machine = runtime.runtime_adapter();
    let registration = machine
        .current_session_registration_witness(sid)
        .await
        .expect("current registration");
    machine
        .retire_scoped_context(&registration, session_target(sid).context_generation)
        .await
        .expect("the registration owner retires the destination");
}

/// A connection whose trusted setup supplies each session's destination binds
/// it at create and stages `mcp/add` only under an authenticated control the
/// native owners prepared for that bound destination. For an unknown
/// session, a persisted write, an extra wire parameter or a destination that
/// is no longer bound, nothing reaches the owners and nothing is staged or
/// spawned.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_jsonl_mcp_add_stages_under_authenticated_control_on_bound_destination() {
    use meerkat_core::mcp_attachment::McpAttachmentRefusal;
    let mut http = Server::start().await;
    let Fixture {
        setup,
        permissions,
        attachments,
        ..
    } = fixture_with(&http, "actual-jsonl-authentication", true).await;
    let markers = tempfile::tempdir().unwrap();
    let marker = markers.path().join("spawned");
    let server_config = sentinel_server(&marker);
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (r, w) = tokio::io::split(server_io);
    let (mut server, runtime) = construct(BufReader::new(r), w, setup).unwrap();
    let server_task = tokio::spawn(async move { server.run().await });
    let (r, mut w) = tokio::io::split(client_io);
    let mut r = BufReader::new(r);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let scenario = async {
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":301,"method":"initialize"}),
        )
        .await;
        let initialized = response(&mut r, &mut w, 301, &effects).await;
        assert_eq!(
            initialized["result"]["methods"],
            json!([
                "initialize",
                "initialized",
                "cancel",
                "session/create",
                "turn/start",
                "mcp/add"
            ])
        );
        // No registration for an unknown session.
        send(&mut w, mcp_add(310, &SessionId::new(), &server_config)).await;
        let unknown = response(&mut r, &mut w, 310, &effects).await;
        assert_attachment_refused(&unknown, McpAttachmentRefusal::NotCurrent);

        let sid = governed_session_after_turn(&mut r, &mut w, &effects, &permissions, &http).await;

        let mut persisted = mcp_add(311, &sid, &server_config);
        persisted["params"]["persisted"] = json!(true);
        send(&mut w, persisted).await;
        assert_unsupported_scope(&response(&mut r, &mut w, 311, &effects).await);

        let mut forged = mcp_add(312, &sid, &server_config);
        forged["params"]["requester"] = json!("forged");
        send(&mut w, forged).await;
        let forged = response(&mut r, &mut w, 312, &effects).await;
        assert_eq!(forged["error"]["code"], error::INVALID_PARAMS, "{forged}");

        assert!(
            attachments.lock().unwrap().is_empty(),
            "no refusal before preparation reaches the native owners"
        );
        // Nothing was staged by the refusals: the boundary starts no server.
        assert_eq!(runtime.mcp_apply_staged_for_test(&sid).await.unwrap(), 0);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!marker.exists(), "a refused add never spawns the server");

        send(&mut w, mcp_add(314, &sid, &server_config)).await;
        let staged = response(&mut r, &mut w, 314, &effects).await;
        assert!(staged.get("error").is_none(), "{staged}");
        assert_eq!(staged["result"]["session_id"], sid.to_string());
        assert_eq!(staged["result"]["operation"], "add");
        assert_eq!(staged["result"]["server_name"], SENTINEL_SERVER);
        assert_eq!(staged["result"]["status"], "staged");
        assert_eq!(staged["result"]["persisted"], false);
        let consults = attachments.lock().unwrap().clone();
        assert!(
            consults.iter().any(|consult| consult.owner == "mandate")
                && consults.iter().any(|consult| consult.owner == "operation")
                && consults
                    .iter()
                    .all(|consult| consult.allowed && consult.server == SENTINEL_SERVER),
            "both native owners allowed the exact server: {consults:?}"
        );
        assert_eq!(*effects.lock().unwrap(), ["read_record"]);
        // The boundary applies the staged intent under its control: the
        // server is actually started.
        assert_eq!(runtime.mcp_apply_staged_for_test(&sid).await.unwrap(), 1);
        wait_for_marker(&marker).await;
        settle_mcp(&runtime, &sid).await;

        // Once the bound destination is withdrawn there is nothing to attach
        // to: refused as not current before any owner decision.
        let before_retired = attachments.lock().unwrap().len();
        retire_destination(&runtime, &sid).await;
        let retired_marker = markers.path().join("retired-spawned");
        let mut retired_server = sentinel_server(&retired_marker);
        retired_server.name = "records-retired".into();
        send(&mut w, mcp_add(315, &sid, &retired_server)).await;
        let unbound = response(&mut r, &mut w, 315, &effects).await;
        assert_attachment_refused(&unbound, McpAttachmentRefusal::NotCurrent);
        // A refused add leaves nothing staged: the next boundary starts no
        // server.
        assert_eq!(runtime.mcp_apply_staged_for_test(&sid).await.unwrap(), 0);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !retired_marker.exists(),
            "a refused add never spawns the server"
        );
        assert_eq!(
            attachments.lock().unwrap().len(),
            before_retired,
            "an unbound destination never reaches the native owners"
        );
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(30), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
}

/// The connection's factory launches its live MCP stdio servers inside the
/// host's confinement: a staged server whose launch the platform backend
/// cannot confine is refused, typed, when it is applied, and never spawns.
#[cfg(all(unix, feature = "mcp"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_jsonl_mcp_add_launches_inside_the_host_confinement() {
    use meerkat_core::confinement::{
        ConfinementRefusal, ConfinementSpec, FilesystemAccess, IpNetworkAccess, PathAccess,
        PlatformBaseline,
    };
    let mut http = Server::start().await;
    let Fixture {
        mut setup,
        permissions,
        ..
    } = fixture_with(&http, "actual-jsonl-authentication", true).await;
    let markers = tempfile::tempdir().unwrap();
    let root = markers.path().canonicalize().unwrap();
    let marker = root.join("spawned");
    // Descendant termination is unsupported on every platform backend.
    let requirement = ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.clone())]),
        write: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.clone())]),
        deny_read: vec![],
        deny_write: vec![],
        network: IpNetworkAccess::Denied,
        unix_connect: vec![],
        require_descendant_termination: true,
    }
    .try_into()
    .unwrap();
    setup.host_confinement = meerkat::HostConfinement::resolve(
        &meerkat_core::ConfinementConfig {
            mcp_stdio: Some(meerkat_core::LaunchConfinement::Custom(
                meerkat_core::CustomConfinement {
                    working_directory: root.clone(),
                    requirement,
                },
            )),
            ..meerkat_core::ConfinementConfig::default()
        },
        None,
    )
    .unwrap();
    let server_config = sentinel_server(&marker);
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (r, w) = tokio::io::split(server_io);
    let (mut server, runtime) = construct(BufReader::new(r), w, setup).unwrap();
    let server_task = tokio::spawn(async move { server.run().await });
    let (r, mut w) = tokio::io::split(client_io);
    let mut r = BufReader::new(r);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let scenario = async {
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":301,"method":"initialize"}),
        )
        .await;
        let initialized = response(&mut r, &mut w, 301, &effects).await;
        assert!(initialized.get("error").is_none(), "{initialized}");
        let sid = governed_session_after_turn(&mut r, &mut w, &effects, &permissions, &http).await;

        send(&mut w, mcp_add(314, &sid, &server_config)).await;
        let staged = response(&mut r, &mut w, 314, &effects).await;
        assert_eq!(staged["result"]["status"], "staged", "{staged}");
        runtime.mcp_apply_staged_for_test(&sid).await.unwrap();
        let deltas = runtime
            .mcp_wait_until_ready_for_test(&sid, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(
            deltas.iter().any(|delta| delta.target == SENTINEL_SERVER
                && delta.phase == meerkat_core::ExternalToolDeltaPhase::Failed
                && delta.confinement_refusal == Some(ConfinementRefusal::UnsupportedRequirement)),
            "the applied launch is refused by the host's confinement: {deltas:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!marker.exists(), "a refused launch never runs unconfined");
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(30), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
}

/// A control whose receipt does not carry the connection's actual
/// authentication is refused, typed, by the mandate owner, on a session the
/// connection bound; the server is never staged or spawned.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_jsonl_mcp_add_unauthenticated_control_is_refused_and_never_spawns() {
    use meerkat_core::mcp_attachment::McpAttachmentRefusal;
    let mut http = Server::start().await;
    let Fixture {
        setup,
        permissions,
        attachments,
        ..
    } = fixture_with(&http, "unverified-authentication", true).await;
    let markers = tempfile::tempdir().unwrap();
    let marker = markers.path().join("spawned");
    let server_config = sentinel_server(&marker);
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (r, w) = tokio::io::split(server_io);
    let (mut server, runtime) = construct(BufReader::new(r), w, setup).unwrap();
    let server_task = tokio::spawn(async move { server.run().await });
    let (r, mut w) = tokio::io::split(client_io);
    let mut r = BufReader::new(r);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let scenario = async {
        let sid = governed_session_after_turn(&mut r, &mut w, &effects, &permissions, &http).await;
        send(&mut w, mcp_add(320, &sid, &server_config)).await;
        let refused = response(&mut r, &mut w, 320, &effects).await;
        assert_attachment_refused(&refused, McpAttachmentRefusal::Denied);
        assert_eq!(
            *attachments.lock().unwrap(),
            [AttachmentConsult {
                owner: "mandate",
                server: SENTINEL_SERVER.into(),
                allowed: false,
            }],
            "the mandate owner refused the unauthenticated control"
        );
        // Nothing was staged: the next boundary starts no server.
        assert_eq!(runtime.mcp_apply_staged_for_test(&sid).await.unwrap(), 0);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !marker.exists(),
            "a refused control never spawns the server"
        );
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(30), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
}

/// A created session whose trusted destination cannot be produced is
/// refused and archived through the staged cleanup path: it is not listed,
/// and its capacity is released, so the next create (whose destination the
/// setup does supply) succeeds under a one-session limit.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_jsonl_create_without_destination_is_refused_and_cleaned_up() {
    let mut http = Server::start().await;
    let Fixture { mut setup, .. } = fixture_with(&http, "actual-jsonl-authentication", true).await;
    setup.config.limits.max_sessions = Some(1);
    let produced = Arc::new(AtomicUsize::new(0));
    setup.session_binder = Some({
        let produced = produced.clone();
        Arc::new(FixtureBinder {
            bound: Arc::default(),
            produce: Arc::new(move |session: &SessionId| {
                (produced.fetch_add(1, Ordering::SeqCst) > 0).then(|| session_target(session))
            }),
        })
    });
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (r, w) = tokio::io::split(server_io);
    let (mut server, runtime) = construct(BufReader::new(r), w, setup).unwrap();
    let server_task = tokio::spawn(async move { server.run().await });
    let (r, mut w) = tokio::io::split(client_io);
    let mut r = BufReader::new(r);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let scenario = async {
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":330,"method":"session/create","params":{
                "prompt":"deferred-seed-s4", "initial_turn":"deferred"
            }}),
        )
        .await;
        let refused = response(&mut r, &mut w, 330, &effects).await;
        assert_unsupported_scope(&refused);
        assert!(
            runtime
                .list_sessions(meerkat_core::service::SessionQuery::default())
                .await
                .unwrap()
                .is_empty(),
            "the unbound session was archived"
        );

        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":331,"method":"session/create","params":{
                "prompt":"deferred-seed-s4", "initial_turn":"deferred"
            }}),
        )
        .await;
        let created = response(&mut r, &mut w, 331, &effects).await;
        assert!(
            created.get("error").is_none(),
            "the refused session released its capacity: {created}"
        );
        assert_eq!(produced.load(Ordering::SeqCst), 2);
        assert_eq!(
            runtime
                .list_sessions(meerkat_core::service::SessionQuery::default())
                .await
                .unwrap()
                .len(),
            1
        );
        // The limit is real: a further create while that session is held is
        // refused for capacity, so the earlier success proves the release.
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":332,"method":"session/create","params":{
                "prompt":"deferred-seed-s4", "initial_turn":"deferred"
            }}),
        )
        .await;
        let over = response(&mut r, &mut w, 332, &effects).await;
        assert!(over.get("result").is_none(), "{over}");
        assert_eq!(
            produced.load(Ordering::SeqCst),
            2,
            "a capacity refusal never reaches the destination producer"
        );
        assert!(http.receiver.bodies.lock().unwrap().is_empty());
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(30), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
}

/// The real stock host composition: a stock-governed realm (the operator's
/// exact grants recorded, the sentinel servers installed in the realm's own
/// config) opened with `RealmGovernance::Stock`, served as governed JSONL
/// through [`stock::stock_governed_jsonl_setup`] with none of them
/// configured for startup. A created session is bound to its canonical stock
/// destination and its agent, whose mandate names every selected server; its
/// prompt is bound through the stock ledger; a governed `mcp/add` of the
/// selected (but unconfigured) installation stages and starts it. An
/// installation with no selected grant, a selected connector-bound
/// installation (this composition has no connector authority) and a server
/// the realm never installed are refused, and none starts.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_governed_realm_binds_sessions_and_stages_its_installation() {
    use meerkat::governed_realm::stock::issuance::{
        DEFAULT_STOCK_AGENT, McpOperations, StockGrantRequest, StockGrantTarget, grant_stock_agent,
    };
    use meerkat::governed_realm::stock::{StockGovernance, StockPrincipals, govern_stock_realm};
    use meerkat::governed_realm::{RealmGovernance, open_governed_realm_persistence_with_provider};
    const REALM: &str = "stock-jsonl";
    let mut http = Server::start().await;
    let root = tempfile::tempdir().unwrap();
    let markers = tempfile::tempdir().unwrap();
    let marker = markers.path().join("spawned");
    let installation = |name: &str, marker: &std::path::Path| {
        let mut server = sentinel_server(marker);
        server.name = name.into();
        // A realm installation carries its explicit operation ceiling; one
        // without a ceiling is never one the realm attaches.
        server.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
        server
    };
    let installed = installation("stock-sentinel", &marker);
    let unselected_marker = markers.path().join("unselected");
    let unselected = installation("unselected-sentinel", &unselected_marker);
    let connector_marker = markers.path().join("connector");
    let mut connector = installation("connector-sentinel", &connector_marker);
    connector.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
        realm_id: REALM.into(),
        slot_id: "mail".into(),
        issuer: "https://issuer.invalid".into(),
        client: "client".into(),
        resource: "https://api.invalid".into(),
        strategy_id: "oidc".into(),
        account: "subject-1".into(),
        operations: [(
            "read".to_string(),
            meerkat_core::mcp_config::McpConnectorOperation {
                required_scopes: ["files.read".to_string()].into(),
                destination_origin: "https://api.invalid".into(),
                read_only: true,
            },
        )]
        .into(),
    });
    let paths = meerkat_store::realm_paths_in(root.path(), REALM);
    let open = |governance: RealmGovernance| {
        open_governed_realm_persistence_with_provider(
            &meerkat::DiskStorageProvider,
            root.path(),
            REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
            governance,
        )
    };
    drop(open(RealmGovernance::Ungoverned).await.unwrap());
    let realm_config = meerkat_core::ConfigRuntime::new(
        Arc::new(meerkat_core::FileConfigStore::new(
            paths.config_path.clone(),
            meerkat_models::canonical(),
        )),
        paths.config_path.with_file_name("config_state.json"),
    );
    for server in [&installed, &unselected, &connector] {
        meerkat_core::McpRealmPersistTarget::new(&realm_config)
            .add(server.clone(), None)
            .await
            .expect("realm installation");
    }
    let realm = RealmId::parse(REALM).unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    govern_stock_realm(root.path(), REALM, &principals)
        .await
        .expect("governed");
    // The operator grants the agent its controller's actual route, the plain
    // installation and the connector-bound one, each as its own exact
    // grant; the unselected installation gets none.
    let client = http_client_in(&http, REALM);
    let selected = client.controller_model_selection().unwrap();
    let pin = {
        use meerkat_core::AgentLlmClient as _;
        Arc::new(meerkat_llm_core::LlmClientAdapter::new(
            Arc::clone(&client),
            selected.model().to_string(),
        ))
        .pin_controller()
        .expect("the controller pin")
    };
    let route = meerkat::governed_realm::stock::controller::controller_route_key(&pin)
        .expect("the controller's actual route");
    for target in [
        StockGrantTarget::Controller {
            routes: [route].into(),
        },
        StockGrantTarget::Mcp {
            server: installed.name.clone(),
            operations: McpOperations::Exact(["read".to_string()].into()),
        },
        StockGrantTarget::Mcp {
            server: connector.name.clone(),
            operations: McpOperations::Exact(["read".to_string()].into()),
        },
    ] {
        grant_stock_agent(
            root.path(),
            REALM,
            &principals,
            &StockGrantRequest {
                agent: DEFAULT_STOCK_AGENT.into(),
                target,
                expires_in_ms: None,
            },
        )
        .await
        .expect("issued and selected");
    }

    let governance = StockGovernance::new(realm.clone(), principals.clone());
    let (_, bundle) = open(RealmGovernance::Stock(Arc::clone(&governance)))
        .await
        .unwrap();
    assert!(governance.has_grant_selection());
    // None of the installations is configured for startup: the selected
    // one is attached live.
    let (config, _) = governed_client_config(&http, &selected, &bundle).await;
    // An explicit stock-governed connection serves no fixed callback tools.
    let setup = stock::stock_governed_jsonl_setup(&governance, config, bundle, client, Vec::new())
        .expect("the stock host's governed JSONL setup");
    http.receiver.direct_answer.store(true, Ordering::SeqCst);
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (r, w) = tokio::io::split(server_io);
    let (mut server, runtime) = construct(BufReader::new(r), w, setup).unwrap();
    let server_task = tokio::spawn(async move { server.run().await });
    let (r, mut w) = tokio::io::split(client_io);
    let mut r = BufReader::new(r);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let scenario = async {
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":340,"method":"session/create","params":{
                "prompt":"deferred-seed-stock", "initial_turn":"deferred"
            }}),
        )
        .await;
        let created = response(&mut r, &mut w, 340, &effects).await;
        assert!(created.get("error").is_none(), "{created}");
        let sid = SessionId::parse(created["result"]["session_id"].as_str().unwrap()).unwrap();

        // An empty-callback governed model turn: the prompt is bound through
        // the stock ledger to the session's stock destination, admitted by
        // the stock owners and answered by the model.
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":341,"method":"turn/start","params":{
                "session_id":sid.to_string(), "prompt":"turn-prompt-stock"
            }}),
        )
        .await;
        let turn = response(&mut r, &mut w, 341, &effects).await;
        assert!(turn.get("error").is_none(), "{turn}");
        assert_eq!(turn["result"]["text"], FINISHED);
        assert_eq!(http.receiver.bodies.lock().unwrap().len(), 1);
        assert!(
            http.receiver.bodies.lock().unwrap()[0]
                .get("tools")
                .and_then(Value::as_array)
                .is_none_or(|tools| tools.iter().all(|tool| tool["name"] != "read_record")),
            "no fixed callback tool is offered"
        );

        // The selected installation, not configured for startup, attaches
        // live under the stock policy for the session's agent: its mandate
        // names it with its own lineage.
        send(&mut w, mcp_add(342, &sid, &installed)).await;
        let staged = response(&mut r, &mut w, 342, &effects).await;
        assert!(staged.get("error").is_none(), "{staged}");
        assert_eq!(staged["result"]["status"], "staged");
        assert_eq!(runtime.mcp_apply_staged_for_test(&sid).await.unwrap(), 1);
        wait_for_marker(&marker).await;
        settle_mcp(&runtime, &sid).await;

        // An installation with no selected grant is refused for the agent.
        send(&mut w, mcp_add(344, &sid, &unselected)).await;
        let unselected_refusal = response(&mut r, &mut w, 344, &effects).await;
        assert_attachment_refused(
            &unselected_refusal,
            meerkat_core::mcp_attachment::McpAttachmentRefusal::Unavailable,
        );
        // A selected connector-bound installation is refused on its
        // descriptor: this composition has no connector authority.
        send(&mut w, mcp_add(345, &sid, &connector)).await;
        let connector_refusal = response(&mut r, &mut w, 345, &effects).await;
        assert_attachment_refused(
            &connector_refusal,
            meerkat_core::mcp_attachment::McpAttachmentRefusal::Unavailable,
        );
        assert_eq!(runtime.mcp_apply_staged_for_test(&sid).await.unwrap(), 0);

        // A server the realm never installed is refused by the same policy.
        let stranger_marker = markers.path().join("stranger");
        let mut stranger = sentinel_server(&stranger_marker);
        stranger.name = "not-installed".into();
        // Ceilinged like an installation, so only the policy refuses it.
        stranger.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
        send(&mut w, mcp_add(343, &sid, &stranger)).await;
        let refused = response(&mut r, &mut w, 343, &effects).await;
        assert!(refused.get("result").is_none(), "{refused}");
        assert_eq!(runtime.mcp_apply_staged_for_test(&sid).await.unwrap(), 0);
        settle_mcp(&runtime, &sid).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!stranger_marker.exists());
        assert!(
            !unselected_marker.exists(),
            "the unselected installation never starts"
        );
        assert!(
            !connector_marker.exists(),
            "the connector-bound installation never starts"
        );
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(60), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
}

/// Without the realm's native work host the stock setup is refused before
/// any runtime or provider work, even with no fixed callback tools.
#[cfg(feature = "mcp")]
#[tokio::test]
async fn stock_governed_jsonl_without_native_host_is_refused() {
    use meerkat::governed_realm::stock::{StockGovernance, StockPrincipals};
    let mut http = Server::start().await;
    let realm = RealmId::parse("stock-jsonl").unwrap();
    let governance = StockGovernance::new(
        realm.clone(),
        StockPrincipals::for_user(&realm, 4242).unwrap(),
    );
    let bundle = meerkat::PersistenceBundle::new(
        Arc::new(meerkat::MemoryStore::new()),
        Arc::new(InMemoryRuntimeStore::new()),
        Arc::new(meerkat::MemoryBlobStore::new()),
    )
    .expect("an ordinary ungoverned persistence owner");
    let client = http_client_in(&http, "stock-jsonl");
    let selected = client.controller_model_selection().unwrap();
    let (config, _) = governed_client_config(&http, &selected, &bundle).await;
    let setup = stock::stock_governed_jsonl_setup(&governance, config, bundle, client, Vec::new())
        .expect("the setup itself carries no owner");
    let (r, w) = tokio::io::duplex(1024);
    assert!(matches!(
        construct(BufReader::new(r), w, setup),
        Err(GovernedJsonlError::Runtime(
            RuntimeDriverError::ControllerReadinessUnavailable {
                reason: meerkat_runtime::traits::ControllerReadinessFailure::UnsupportedScope,
            }
        ))
    ));
    assert!(http.receiver.bodies.lock().unwrap().is_empty());
    http.reap().await;
}

/// Process termination ends a governed connection that is still open, as it
/// does the ordinary stdio server: it stops reading, drains and returns.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_jsonl_stops_and_drains_on_termination() {
    let mut http = Server::start().await;
    let Fixture { setup, .. } = fixture_with(&http, "actual-jsonl-authentication", true).await;
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (r, w) = tokio::io::split(server_io);
    let (terminate, terminated) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(serve_governed_jsonl_until(
        BufReader::new(r),
        w,
        setup,
        async move {
            let _ = terminated.await;
        },
    ));
    let (r, mut w) = tokio::io::split(client_io);
    let mut r = BufReader::new(r);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    send(
        &mut w,
        json!({"jsonrpc":"2.0","id":350,"method":"initialize"}),
    )
    .await;
    let initialized = response(&mut r, &mut w, 350, &effects).await;
    assert!(initialized.get("error").is_none(), "{initialized}");
    // The client keeps its end open; only termination ends the connection.
    terminate.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), served)
        .await
        .expect("termination drains the governed connection promptly")
        .expect("the server task completes");
    assert!(result.is_ok(), "{result:?}");
    drop(w);
    http.reap().await;
}

/// Termination while a governed turn is admitted and running drains it the
/// ordinary way: the connection stops reading, its responses are rejected,
/// the in-flight request is settled and the runtime shuts down cleanly with
/// the admitted run, instead of hanging on it or failing.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn governed_jsonl_termination_drains_an_active_turn() {
    let mut http = Server::start().await;
    let Fixture {
        setup, permissions, ..
    } = fixture_with(&http, "actual-jsonl-authentication", true).await;
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (r, w) = tokio::io::split(server_io);
    let (terminate, terminated) = tokio::sync::oneshot::channel::<()>();
    let served = tokio::spawn(serve_governed_jsonl_until(
        BufReader::new(r),
        w,
        setup,
        async move {
            let _ = terminated.await;
        },
    ));
    let (r, mut w) = tokio::io::split(client_io);
    let mut r = BufReader::new(r);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    send(
        &mut w,
        json!({"jsonrpc":"2.0","id":360,"method":"session/create","params":{
            "prompt":"deferred-seed-drain", "initial_turn":"deferred"
        }}),
    )
    .await;
    let created = response(&mut r, &mut w, 360, &effects).await;
    assert!(created.get("error").is_none(), "{created}");
    let sid = SessionId::parse(created["result"]["session_id"].as_str().unwrap()).unwrap();
    permissions.lock().unwrap().push(InvocationPermission {
        runtime: LogicalRuntimeId::for_session(&sid),
        requester: principal("requester"),
        ingress: principal("ingress"),
    });
    send(
        &mut w,
        json!({"jsonrpc":"2.0","id":361,"method":"turn/start","params":{
            "session_id":sid.to_string(), "prompt":"turn-prompt-drain"
        }}),
    )
    .await;
    // Serve the turn's callbacks until its second model request is held: the
    // input is admitted and its run is in flight.
    let turn = response(&mut r, &mut w, 361, &effects);
    tokio::pin!(turn);
    tokio::select! {
        result = &mut turn => panic!("the turn completed before the held model request: {result}"),
        () = http.receiver.second_request.notified() => {}
    }
    terminate.send(()).unwrap();
    http.receiver.finish.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(20), served)
        .await
        .expect("termination drains the active turn and returns")
        .expect("the server task completes");
    assert!(result.is_ok(), "the runtime shut down cleanly: {result:?}");
    http.reap().await;
}

/// A stock-governed realm with per-target grants, served as governed JSONL
/// with the realm's installations as configured servers in its effective
/// config (as `rkat-rpc` reads it). The stock composition carries their full
/// descriptors into each session's build and binds the session's agent on
/// its current registration before the promotion builds it: the configured
/// installation granted to the agent connects; the installed one with no
/// selected grant is refused at its connection while the session continues;
/// a granted connector-bound installation is refused before any connection
/// (the composition has no connector authority yet); and once the realm's
/// configuration moved past the generation it opened at, the granted
/// installation is refused too.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_governed_jsonl_connects_the_agents_configured_servers() {
    stock_configured_servers_scenario(false).await;
}

/// The same stock composition with the host's connector authority wired (as
/// `rkat-rpc` composes it for a realm with connector-bound servers): every
/// session's runtime carries the connector owner, and the granted
/// connector-bound installation connects in the promoted build like the
/// plain one. The ungranted installation and a moved generation are still
/// refused.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_governed_jsonl_connects_connector_bound_servers_with_the_connector_authority() {
    stock_configured_servers_scenario(true).await;
}

#[cfg(feature = "mcp")]
async fn stock_configured_servers_scenario(connector_wired: bool) {
    use meerkat::governed_realm::stock::issuance::{
        DEFAULT_STOCK_AGENT, McpOperations, StockGrantRequest, StockGrantTarget, grant_stock_agent,
    };
    use meerkat::governed_realm::stock::{StockGovernance, StockPrincipals, govern_stock_realm};
    use meerkat::governed_realm::{RealmGovernance, open_governed_realm_persistence_with_provider};
    const REALM: &str = "stock-agent";
    let mut http = Server::start().await;
    let root = tempfile::tempdir().unwrap();
    let markers = tempfile::tempdir().unwrap();
    // The connector's issuer: a real OAuth/OIDC fixture the authority
    // logs in against.
    let issuer = meerkat::test_fixtures::mcp_oauth::McpOAuthFixture::spawn()
        .await
        .unwrap();
    let marker = |name: &str| markers.path().join(name);
    let installation = |name: &str| {
        let mut server = sentinel_server(&marker(name));
        server.name = name.into();
        server.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
        server
    };
    let granted = installation("granted");
    let ungranted = installation("ungranted");
    // The connector-bound installation: a protocol-capable MCP server that
    // records the binding it was started with and every tools/call line it
    // receives (its params carry the call context the session prepared).
    let calls = markers.path().join("connector-calls.log");
    let mut connector = installation("connector");
    if let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut connector.transport {
        stdio.args = vec![
            "-c".into(),
            format!(
                r#"printf '%s' "$MEERKAT_CONNECTOR_BINDING" > '{marker}'
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"connector","version":"0"}}}}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"read","inputSchema":{{"type":"object"}}}}]}}}}\n' "$id" ;;
    *'"method":"tools/call"'*) printf '%s\n' "$line" >> '{calls}'; printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"connector-read"}}]}}}}\n' "$id" ;;
  esac
done
"#,
                marker = marker("connector").display(),
                calls = calls.display(),
            ),
        ];
    }
    connector.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
        realm_id: REALM.into(),
        slot_id: "mail".into(),
        issuer: issuer.base().into(),
        client: "client".into(),
        resource: "https://api.invalid".into(),
        strategy_id: "oidc-userinfo-v1".into(),
        account: meerkat::test_fixtures::mcp_oauth::SUBJECT.into(),
        operations: [(
            "read".to_string(),
            meerkat_core::mcp_config::McpConnectorOperation {
                required_scopes: ["openid".to_string()].into(),
                destination_origin: "https://api.invalid".into(),
                read_only: true,
            },
        )]
        .into(),
    });
    let paths = meerkat_store::realm_paths_in(root.path(), REALM);
    let open = |governance: RealmGovernance| {
        open_governed_realm_persistence_with_provider(
            &meerkat::DiskStorageProvider,
            root.path(),
            REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
            governance,
        )
    };
    drop(open(RealmGovernance::Ungoverned).await.unwrap());
    let realm_config = meerkat_core::ConfigRuntime::new(
        Arc::new(meerkat_core::FileConfigStore::new(
            paths.config_path.clone(),
            meerkat_models::canonical(),
        )),
        paths.config_path.with_file_name("config_state.json"),
    );
    for server in [&granted, &ungranted, &connector] {
        meerkat_core::McpRealmPersistTarget::new(&realm_config)
            .add(server.clone(), None)
            .await
            .expect("realm installation");
    }
    let realm = RealmId::parse(REALM).unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    govern_stock_realm(root.path(), REALM, &principals)
        .await
        .expect("governed");
    // The operator grants the agent its controller's actual route, the plain
    // `granted` installation and the connector-bound one.
    let client = http_client_in(&http, REALM);
    let selected = client.controller_model_selection().unwrap();
    let pin = {
        use meerkat_core::AgentLlmClient as _;
        Arc::new(meerkat_llm_core::LlmClientAdapter::new(
            Arc::clone(&client),
            selected.model().to_string(),
        ))
        .pin_controller()
        .expect("the controller pin")
    };
    let route = meerkat::governed_realm::stock::controller::controller_route_key(&pin)
        .expect("the controller's actual route");
    for target in [
        StockGrantTarget::Controller {
            routes: [route].into(),
        },
        StockGrantTarget::Mcp {
            server: granted.name.clone(),
            operations: McpOperations::Exact(["read".to_string()].into()),
        },
        StockGrantTarget::Mcp {
            server: connector.name.clone(),
            operations: McpOperations::Exact(["read".to_string()].into()),
        },
    ] {
        grant_stock_agent(
            root.path(),
            REALM,
            &principals,
            &StockGrantRequest {
                agent: DEFAULT_STOCK_AGENT.into(),
                target,
                expires_in_ms: None,
            },
        )
        .await
        .expect("issued and selected");
    }

    let governance = StockGovernance::new(realm.clone(), principals.clone());
    let (_, bundle) = open(RealmGovernance::Stock(Arc::clone(&governance)))
        .await
        .unwrap();
    let (mut config, _) = governed_client_config(&http, &selected, &bundle).await;
    // The realm's effective config lists its installations as configured
    // servers, with their full descriptors.
    config.tools.mcp_servers = vec![granted.clone(), ungranted.clone(), connector.clone()];
    let connector_authority = connector_wired.then(|| {
        meerkat::HostAuthService::new(
            meerkat::ProviderAuthPersistence::new(
                Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
                Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
            ),
            bundle.runtime_adapter().provider_auth_runtime_authority(),
        )
        .connector_oauth_authority()
        .expect("the host's connector authority")
    });
    if let Some(authority) = &connector_authority {
        // The connector account is authorized: one real login against the
        // issuer for the installation's own slot, account and scopes.
        let binding = connector.connector.as_ref().unwrap();
        let slot = meerkat_core::connection::CredentialAccountRef {
            realm: realm.clone(),
            account: meerkat_core::connection::CredentialAccountId::parse(&binding.slot_id)
                .unwrap(),
        };
        let redirect = "http://127.0.0.1:1/connector/oauth/callback";
        let started = authority
            .login_start(
                &meerkat::ConnectorOAuthTarget {
                    slot: slot.clone(),
                    issuer: binding.issuer.clone(),
                    client: binding.client.clone(),
                    resource: binding.resource.clone(),
                    scopes: ["openid".to_string()].into(),
                    strategy_id: binding.strategy_id.clone(),
                    account: meerkat_providers::connector_oauth::AccountSelection::Known(
                        binding.account.clone(),
                    ),
                    client_auth_profile: None,
                },
                redirect,
            )
            .await
            .expect("connector login starts");
        authority
            .login_complete(
                &slot,
                meerkat::ConnectorOAuthCallback {
                    redirect_uri: redirect.into(),
                    state: started.state,
                    code: meerkat::test_fixtures::mcp_oauth::CODE_CANARY.into(),
                    host_attempt_handle: None,
                },
            )
            .await
            .expect("the connector account is authorized");
    }
    let setup = stock::stock_governed_jsonl_setup_with_connector_authority(
        &governance,
        config,
        bundle,
        client,
        Vec::new(),
        connector_authority,
    )
    .expect("the stock host's governed JSONL setup");
    http.receiver.direct_answer.store(true, Ordering::SeqCst);
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (r, w) = tokio::io::split(server_io);
    let (mut server, runtime) = construct(BufReader::new(r), w, setup)
        .expect("the stock composition carries the configured servers");
    let server_task = tokio::spawn(async move { server.run().await });
    let (r, mut w) = tokio::io::split(client_io);
    let mut r = BufReader::new(r);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let scenario = async {
        // Create a deferred session and promote it with a governed model
        // turn on the agent's controller grant; returns the turn response.
        async fn create_and_turn(
            w: &mut (impl tokio::io::AsyncWrite + Unpin),
            r: &mut (impl tokio::io::AsyncBufRead + Unpin),
            effects: &Arc<Mutex<Vec<String>>>,
            id: u64,
            label: &str,
        ) -> (SessionId, Value) {
            send(
                w,
                json!({"jsonrpc":"2.0","id":id,"method":"session/create","params":{
                    "prompt":format!("deferred-seed-{label}"), "initial_turn":"deferred"
                }}),
            )
            .await;
            let created = response(r, w, id, effects).await;
            assert!(created.get("error").is_none(), "{created}");
            let sid = created["result"]["session_id"].as_str().unwrap().to_owned();
            send(
                w,
                json!({"jsonrpc":"2.0","id":id + 1,"method":"turn/start","params":{
                    "session_id":sid, "prompt":format!("turn-prompt-{label}")
                }}),
            )
            .await;
            let turn = response(r, w, id + 1, effects).await;
            (SessionId::parse(&sid).unwrap(), turn)
        }

        // The granted installation connects in the promoted build; the
        // ungranted one is refused at its connection and the turn completes.
        let (sid, turn) = create_and_turn(&mut w, &mut r, &effects, 352, "first").await;
        assert!(turn.get("error").is_none(), "{turn}");
        assert_eq!(turn["result"]["text"], FINISHED);
        assert_eq!(runtime.has_connector_authority_for_test(), connector_wired);
        wait_for_marker(&marker("granted")).await;
        if connector_wired {
            // The connector-bound installation connects like the plain one,
            // started with the granted installation's startup projection of
            // its binding (issuer, strategy, subject, operations).
            wait_for_marker(&marker("connector")).await;
            assert_eq!(
                std::fs::read_to_string(marker("connector")).unwrap(),
                connector
                    .connector
                    .as_ref()
                    .unwrap()
                    .startup_projection()
                    .to_string(),
                "the granted installation's own binding"
            );
        }
        settle_mcp(&runtime, &sid).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!marker("ungranted").exists(), "no selected grant, no spawn");
        if connector_wired {
            // A real granted call: the model calls the connector server's
            // granted raw operation; the session prepares its call context
            // from the stored authorized credential for the installation's
            // own staged binding, and the server receives it.
            *http.receiver.tool_call.lock().unwrap() = Some("read".into());
            send(
                &mut w,
                json!({"jsonrpc":"2.0","id":360,"method":"turn/start","params":{
                    "session_id":sid.to_string(), "prompt":"call the connector"
                }}),
            )
            .await;
            let called = response(&mut r, &mut w, 360, &effects).await;
            assert!(called.get("error").is_none(), "{called}");
            assert_eq!(called["result"]["text"], FINISHED);
            let lines = std::fs::read_to_string(&calls).expect("the server received a call");
            let lines: Vec<&str> = lines.lines().collect();
            assert_eq!(lines.len(), 1, "exactly the granted call: {lines:?}");
            let call: Value = serde_json::from_str(lines[0]).unwrap();
            assert_eq!(call["params"]["name"], "read");
            let credential = &call["params"]["_meta"][meerkat::CONNECTOR_CREDENTIAL_META_KEY];
            let binding = connector.connector.as_ref().unwrap();
            assert_eq!(
                credential["authorization"],
                format!(
                    "Bearer {}",
                    meerkat::test_fixtures::mcp_oauth::ACCESS_CANARY
                ),
                "the stored authorized credential"
            );
            assert_eq!(
                credential["account"],
                json!({
                    "issuer": binding.issuer,
                    "strategy_id": binding.strategy_id,
                    "subject": binding.account,
                }),
                "the granted binding's account"
            );
            assert_eq!(credential["destination_origin"], "https://api.invalid");
            assert_eq!(credential["scopes"], json!(["openid"]));
            // The tool result the model saw is the server's reply.
            let bodies = http.receiver.bodies.lock().unwrap().clone();
            let result = bodies
                .iter()
                .flat_map(|body| body["messages"].as_array().cloned().unwrap_or_default())
                .flat_map(|message| message["content"].as_array().cloned().unwrap_or_default())
                .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == SINGLE_CALL)
                .expect("the call's tool result reached the model");
            assert_ne!(result["is_error"], json!(true), "{result}");
        } else {
            assert!(
                !marker("connector").exists(),
                "an unsupported connector-bound server is refused before connection"
            );
            // A live add of the same granted connector-bound server is
            // refused per call: the composition selected no grant lineage
            // for it.
            send(&mut w, mcp_add(360, &sid, &connector)).await;
            let refused = response(&mut r, &mut w, 360, &effects).await;
            assert!(refused.get("result").is_none(), "{refused}");
            assert!(
                refused["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("unavailable")),
                "{refused}"
            );
            assert_eq!(runtime.mcp_apply_staged_for_test(&sid).await.unwrap(), 0);
            settle_mcp(&runtime, &sid).await;
            assert!(!marker("connector").exists(), "the live add never spawns");
        }

        // The realm's configuration moves past the generation it opened at:
        // a later session's granted installation is no longer current.
        meerkat_core::McpRealmPersistTarget::new(&realm_config)
            .add(installation("later"), None)
            .await
            .expect("a later installation");
        std::fs::remove_file(marker("granted")).unwrap();
        if connector_wired {
            std::fs::remove_file(marker("connector")).unwrap();
        }
        let (sid, turn) = create_and_turn(&mut w, &mut r, &effects, 354, "second").await;
        assert!(turn.get("error").is_none(), "{turn}");
        assert_eq!(turn["result"]["text"], FINISHED);
        settle_mcp(&runtime, &sid).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!marker("granted").exists(), "not current, no spawn");
        assert!(!marker("ungranted").exists());
        assert!(!marker("connector").exists());
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(60), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
}

/// The `tool_config_changed` payloads `session` reported, in order.
#[cfg(feature = "mcp")]
fn tool_config_changes(
    notifications: &SessionNotifications,
    session: &SessionId,
) -> Vec<meerkat_core::ToolConfigChangedPayload> {
    notifications
        .lock()
        .unwrap()
        .iter()
        .filter(|(route, _)| route == session)
        .filter_map(|(_, event)| match &event.payload {
            AgentEvent::ToolConfigChanged { payload } => Some(payload.clone()),
            _ => None,
        })
        .collect()
}

/// Whether `changes` carry a restored live attachment's exact lifecycle
/// event: server `target`'s Add, pending, re-staged with no restore outcome
/// (a successful restore carries no restore field).
#[cfg(feature = "mcp")]
fn restaged_add(changes: &[meerkat_core::ToolConfigChangedPayload], target: &str) -> bool {
    changes.iter().any(|payload| {
        payload.operation == meerkat_core::ToolConfigChangeOperation::Add
            && payload.target == target
            && payload.status_text() == "pending"
            && !matches!(
                payload.status_info(),
                meerkat_core::ToolConfigChangeStatus::ExternalToolDelta {
                    restore: Some(_),
                    ..
                }
            )
    })
}

/// The restore outcomes in `changes`, as (server, wire outcome) pairs.
#[cfg(feature = "mcp")]
fn restore_refusals(changes: &[meerkat_core::ToolConfigChangedPayload]) -> Vec<(String, String)> {
    changes
        .iter()
        .filter_map(|payload| match payload.status_info() {
            meerkat_core::ToolConfigChangeStatus::ExternalToolDelta {
                restore: Some(restore),
                ..
            } => Some((
                payload.target.clone(),
                serde_json::to_value(restore).ok()?.as_str()?.to_owned(),
            )),
            _ => None,
        })
        .collect()
}

/// The terminal events of `session`'s runs, in order: a completed run's
/// result text, or a failed run's whole event.
#[cfg(feature = "mcp")]
fn run_terminals(
    notifications: &SessionNotifications,
    session: &SessionId,
) -> Vec<Result<String, String>> {
    notifications
        .lock()
        .unwrap()
        .iter()
        .filter(|(route, _)| route == session)
        .filter_map(|(_, event)| match &event.payload {
            AgentEvent::RunCompleted { result, .. } => Some(Ok(result.clone())),
            failed @ AgentEvent::RunFailed { .. } => Some(Err(format!("{failed:?}"))),
            _ => None,
        })
        .collect()
}

/// A governed (stock) host restart: the stock realm's installation, a
/// configured server of the realm's agent, is also attached live to the
/// original session under the stock policy (a real `mcp/add`, accepted and
/// applied), the whole host (runtime, connection and persistence) is dropped,
/// and a new stock host is opened on the same realm root. The resumed
/// original session's durable attachment is restored only through a fresh
/// governed control prepared for the new connection's binding: with the
/// realm's grants still current, the original's resumed turn re-stages the
/// server's exact Add (pending); with the installation's grant revoked while
/// the host was down, that server's restore is refused with a typed `denied`
/// outcome, nothing launches, and the session stays usable. The sibling ran
/// its own first turn on the first host, so it is a durable session too; it
/// restores nothing. Configured startup is legitimate for the sibling; the
/// original's durable record attaches the server live, so the new host's
/// configured startup leaves that server to the original's restore. Neither
/// the spawn marker nor the sibling's startup proves a restore. Both
/// hosts reopen the same file credential store as their attachment
/// identity-key custody, as two host processes on one machine would, so the
/// new host can prove the original's recorded target; it supplies no
/// credential.
///
/// With `boundary_only`, the scenario stops once the new host has opened the
/// realm: the first host's close boundary and an immediate reopen, with no
/// session served on the new host.
#[cfg(feature = "mcp")]
async fn stock_governed_restart(revoke_while_down: bool, boundary_only: bool) {
    use meerkat::governed_realm::stock::issuance::{
        DEFAULT_STOCK_AGENT, McpOperations, StockGrantRequest, StockGrantTarget, grant_stock_agent,
    };
    use meerkat::governed_realm::stock::{StockGovernance, StockPrincipals, govern_stock_realm};
    use meerkat::governed_realm::{RealmGovernance, open_governed_realm_persistence_with_provider};
    const REALM: &str = "stock-restart";
    let mut http = Server::start().await;
    let root = tempfile::tempdir().unwrap();
    let markers = tempfile::tempdir().unwrap();
    let marker = markers.path().join("spawned");
    let mut installed = sentinel_server(&marker);
    installed.name = "stock-sentinel".into();
    installed.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
    let paths = meerkat_store::realm_paths_in(root.path(), REALM);
    let open = |governance: RealmGovernance| {
        open_governed_realm_persistence_with_provider(
            &meerkat::DiskStorageProvider,
            root.path(),
            REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
            governance,
        )
    };
    drop(open(RealmGovernance::Ungoverned).await.unwrap());
    let realm_config = meerkat_core::ConfigRuntime::new(
        Arc::new(meerkat_core::FileConfigStore::new(
            paths.config_path.clone(),
            meerkat_models::canonical(),
        )),
        paths.config_path.with_file_name("config_state.json"),
    );
    meerkat_core::McpRealmPersistTarget::new(&realm_config)
        .add(installed.clone(), None)
        .await
        .expect("realm installation");
    let realm = RealmId::parse(REALM).unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    govern_stock_realm(root.path(), REALM, &principals)
        .await
        .expect("governed");
    let restore_owner = || {
        meerkat_authorization::grants::LocalGrantAuthority::restore(
            Arc::new(meerkat_authorization::grants::FileGrantOwnerStore::new(
                meerkat_store::grant_owner_store_path(&paths.root),
            )),
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("the operator restores the closed realm's owner")
    };
    // The operator grants the agent its controller's actual route and the
    // installation, each as its own exact grant, as the other stock tests
    // do. Both hosts serve through this one client: a route key includes the
    // client's auth binding, and `http_client_in` mints a fresh binding per
    // client.
    let client = http_client_in(&http, REALM);
    let selected = client.controller_model_selection().unwrap();
    let pin = {
        use meerkat_core::AgentLlmClient as _;
        Arc::new(meerkat_llm_core::LlmClientAdapter::new(
            Arc::clone(&client),
            selected.model().to_string(),
        ))
        .pin_controller()
        .expect("the controller pin")
    };
    let route = meerkat::governed_realm::stock::controller::controller_route_key(&pin)
        .expect("the controller's actual route");
    let mut issued = Vec::new();
    for target in [
        StockGrantTarget::Controller {
            routes: [route].into(),
        },
        StockGrantTarget::Mcp {
            server: installed.name.clone(),
            operations: McpOperations::Exact(["read".to_string()].into()),
        },
    ] {
        let outcome = grant_stock_agent(
            root.path(),
            REALM,
            &principals,
            &StockGrantRequest {
                agent: DEFAULT_STOCK_AGENT.into(),
                target,
                expires_in_ms: None,
            },
        )
        .await
        .expect("issued and selected");
        issued.push(outcome.issued);
    }
    // The grant the agent's selection names for the installation.
    let installation_grant = issued.pop().expect("the installation's grant");
    http.receiver.direct_answer.store(true, Ordering::SeqCst);

    // The realm's attachment identity-key custody: one temporary file
    // credential store, which each host reopens with its own refresh
    // authority. It supplies identity custody only, never a credential.
    let credentials = tempfile::tempdir().unwrap();
    // One stock host on the realm root: its runtime and a JSONL connection,
    // plus a weak handle on the realm runtime store its machine holds.
    let start_host = || async {
        let governance = StockGovernance::new(realm.clone(), principals.clone());
        let (_, bundle) = open(RealmGovernance::Stock(Arc::clone(&governance)))
            .await
            .expect("the host opens the realm: no earlier host keeps its execution custody");
        let store = Arc::downgrade(&bundle.runtime_store());
        let client = Arc::clone(&client);
        let selected = client.controller_model_selection().unwrap();
        let (mut config, _) = governed_client_config(&http, &selected, &bundle).await;
        // The realm's effective config lists its installation, with its full
        // descriptor, as a configured server: the stock binder selects the
        // agent's grant lineage for it by that name.
        config.tools.mcp_servers = vec![installed.clone()];
        let mut setup =
            stock::stock_governed_jsonl_setup(&governance, config, bundle, client, Vec::new())
                .expect("the stock host's governed JSONL setup");
        setup.mcp_attachment_identity_key_custody = Some(
            meerkat::TokenStoreBackend::File {
                root: credentials.path().to_path_buf(),
            }
            .open_with_refresh_authority()
            .expect("the host reopens the realm's identity-key custody"),
        );
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (r, w) = tokio::io::split(server_io);
        let (mut server, runtime) = construct(BufReader::new(r), w, setup).unwrap();
        let server_task = tokio::spawn(async move { server.run().await });
        let (r, w) = tokio::io::split(client_io);
        (BufReader::new(r), w, server_task, runtime, store)
    };
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();

    // First host: create the original and a sibling, attach the realm's
    // installation to the original only.
    let (mut r, mut w, mut server_task, runtime, first_store) = start_host().await;
    // Instantaneous sample: the first host's RPC runtime holders as it starts
    // serving, this test's own included.
    let runtime_strong_after_start = Arc::strong_count(&runtime);
    // Taken before any shutdown: the server's own shutdown at EOF empties the
    // runtime's mob state slot and retires its delivery owner. A weak handle
    // on the actual mob state, whose two provider closures name the RPC
    // runtime (router.rs), and the delivery owner's pass channel, which
    // closes once its task is gone. The owner holds the runtime weakly, and
    // strongly only inside a pass.
    #[cfg(feature = "mob")]
    let first_mob_state = runtime.mob_state().map(|state| Arc::downgrade(&state));
    #[cfg(not(feature = "mob"))]
    let first_mob_state: Option<std::sync::Weak<()>> = None;
    let delivery_passes = runtime.subscribe_job_delivery_passes();
    let first = async {
        let mut sessions = Vec::new();
        for (request, prompt) in [(350, "original-seed"), (351, "sibling-seed")] {
            send(
                &mut w,
                json!({"jsonrpc":"2.0","id":request,"method":"session/create","params":{
                    "prompt":prompt, "initial_turn":"deferred"
                }}),
            )
            .await;
            let created = response(&mut r, &mut w, request, &effects).await;
            assert!(created.get("error").is_none(), "{created}");
            sessions
                .push(SessionId::parse(created["result"]["session_id"].as_str().unwrap()).unwrap());
        }
        let (original, sibling) = (sessions[0].clone(), sessions[1].clone());
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":352,"method":"turn/start","params":{
                "session_id":original.to_string(), "prompt":"before-restart"
            }}),
        )
        .await;
        let turn = response(&mut r, &mut w, 352, &effects).await;
        assert!(turn.get("error").is_none(), "{turn}");
        send(&mut w, mcp_add(353, &original, &installed)).await;
        let staged = response(&mut r, &mut w, 353, &effects).await;
        assert!(staged.get("error").is_none(), "{staged}");
        assert_eq!(
            runtime.mcp_apply_staged_for_test(&original).await.unwrap(),
            1
        );
        wait_for_marker(&marker).await;
        settle_mcp(&runtime, &original).await;
        // The sibling runs its own first turn on this host: it consumes its
        // deferred seed once and becomes a durable session the next host
        // resumes. It attaches nothing live.
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":354,"method":"turn/start","params":{
                "session_id":sibling.to_string(), "prompt":"sibling-before-restart"
            }}),
        )
        .await;
        let turn = response(&mut r, &mut w, 354, &effects).await;
        assert!(turn.get("error").is_none(), "{turn}");
        assert_eq!(turn["result"]["text"], FINISHED, "{turn}");
        (original, sibling)
    };
    let (original, sibling) = tokio::time::timeout(Duration::from_secs(60), first)
        .await
        .expect("the first host's scenario");

    // The host goes away: connection, runtime and persistence. Its server
    // task must complete successfully once the connection closes. Borrow the
    // JoinHandle so a timeout never detaches it: on failure it is aborted and
    // joined, and the runtime is still shut down, before the test fails.
    drop(w);
    drop(r);
    let completion = match tokio::time::timeout(Duration::from_secs(30), &mut server_task).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(other) => Err(format!(
            "the first host's JSONL server completion: {other:?}"
        )),
        Err(_) => {
            server_task.abort();
            let joined = tokio::time::timeout(Duration::from_secs(2), &mut server_task).await;
            Err(format!(
                "the first host's JSONL server did not stop within its bound; abort/join: {joined:?}"
            ))
        }
    };
    // Instantaneous samples after the server's own shutdown at EOF and before
    // this test's explicit second one: the delivery owner's pass generation
    // and the RPC runtime's holders, this test's own included.
    let delivery_generation_before_shutdown = delivery_passes
        .as_ref()
        .map(|passes| passes.borrow().generation);
    let runtime_strong_before_second_shutdown = Arc::strong_count(&runtime);
    let shutdown = runtime.try_shutdown().await;
    // The two runtime-owned slots release_mob_state empties, read right after
    // the shutdown.
    #[cfg(feature = "mob")]
    let mob_state_slot_empty = runtime.mob_state().is_none();
    #[cfg(not(feature = "mob"))]
    let mob_state_slot_empty = true;
    let mob_tools_slot_empty = runtime
        .builder_mob_tools_slot
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_none();
    // Weak handles on the first host's owners. The RPC completion and idle
    // cleanup tasks each hold the RPC runtime itself, so its count covers them.
    let runtime_strong_after_shutdown = Arc::strong_count(&runtime);
    let first_runtime = Arc::downgrade(&runtime);
    let first_machine = Arc::downgrade(&runtime.runtime_adapter());
    let first_auth_lease = Arc::downgrade(
        &runtime
            .runtime_adapter()
            .generated_auth_lease_handle()
            .clone_handle(),
    );
    drop(runtime);
    if let Err(failure) = completion {
        panic!("{failure}; runtime shutdown: {shutdown:?}");
    }
    shutdown.expect("the first host shuts down");
    // The close boundary, observed once, before the reopen. The reopen needs
    // the realm's execution custody, which the first host's RPC runtime, its
    // native machine and the machine's credential lifecycle handle each keep
    // while anything retains them. The runtime store, the mob state and the
    // delivery owner are reported with them.
    let runtime_strong = first_runtime.strong_count();
    let machine_strong = first_machine.strong_count();
    let auth_lease_strong = first_auth_lease.strong_count();
    let store_strong = first_store.strong_count();
    let mob_state_strong = first_mob_state.as_ref().map(std::sync::Weak::strong_count);
    let delivery_at_boundary = delivery_passes
        .as_ref()
        .map(|passes| (passes.has_changed().is_ok(), passes.borrow().generation));
    let boundary = format!(
        "instantaneous samples: rpc runtime strong {runtime_strong} (with this test's own: \
         after start_host {runtime_strong_after_start}, after server shutdown, before the \
         explicit second shutdown {runtime_strong_before_second_shutdown}, after the explicit \
         second shutdown {runtime_strong_after_shutdown}); native machine strong \
         {machine_strong}; machine auth lease handle strong {auth_lease_strong}; runtime store \
         strong {store_strong}; mob state strong {mob_state_strong:?} (weak taken after \
         start_host; after the explicit second shutdown: mob_state slot empty \
         {mob_state_slot_empty}, builder mob tools slot empty {mob_tools_slot_empty}); \
         delivery owner (pass sender open, pass generation) {delivery_at_boundary:?} \
         (subscribed after start_host), generation after server shutdown, before the explicit \
         second shutdown {delivery_generation_before_shutdown:?}"
    );
    eprintln!("first host at the close boundary: {boundary}");
    assert!(
        runtime_strong == 0 && machine_strong == 0 && auth_lease_strong == 0,
        "the first host keeps execution custody owners past its shutdown and drop: {boundary}"
    );
    std::fs::remove_file(&marker).expect("reset the spawn marker");

    if revoke_while_down {
        // The operator revokes the installation's grant while no host runs,
        // under the realm's execution custody; the agent's selection still
        // names it.
        let (_, bundle) = open(RealmGovernance::GovernedWithoutOwners).await.unwrap();
        let owner = restore_owner();
        let machine = bundle.runtime_adapter();
        let mut custody = machine
            .try_controller_grant_mutation()
            .expect("the realm's grant custody");
        owner
            .revoke(&principals.operator, &installation_grant, &mut custody)
            .expect("revoked while down");
        drop(custody);
        drop(bundle);
    }

    // A new host on the same realm root resumes the sessions. Configured
    // startup of the installation is legitimate there for the sibling, and
    // leaves the server to the original's restore, so neither the spawn
    // marker nor a sibling that never starts it proves a restore: the
    // original's restored live attachment is its own exact re-staged Add of
    // the server, pending at its resumed turn's boundary.
    let (mut r, mut w, server_task, _runtime, _) = start_host().await;
    if boundary_only {
        // The new host opened the realm with no Busy: nothing of the first
        // host kept its execution custody. It closes without serving.
        let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
        finish_scenario(Ok(Ok(())), cleanup_failures);
        return;
    }
    let notifications: SessionNotifications = Arc::default();
    let second = async {
        // The sibling has nothing to restore; its resumed run completes.
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":360,"method":"turn/start","params":{
                "session_id":sibling.to_string(), "prompt":"sibling-after-restart"
            }}),
        )
        .await;
        let turn = response_with_events(&mut r, &mut w, 360, &effects, Some(&notifications)).await;
        assert!(turn.get("error").is_none(), "{turn}");
        assert_eq!(turn["result"]["text"], FINISHED, "{turn}");
        assert_eq!(
            run_terminals(&notifications, &sibling),
            vec![Ok(FINISHED.to_owned())],
            "the sibling's resumed run completes"
        );
        let sibling_changes = tool_config_changes(&notifications, &sibling);
        assert!(
            !restaged_add(&sibling_changes, &installed.name),
            "the sibling restores no attachment: {sibling_changes:?}"
        );
        assert!(
            restore_refusals(&sibling_changes).is_empty(),
            "the sibling has no restore outcome: {sibling_changes:?}"
        );

        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":361,"method":"turn/start","params":{
                "session_id":original.to_string(), "prompt":"after-restart"
            }}),
        )
        .await;
        let turn = response_with_events(&mut r, &mut w, 361, &effects, Some(&notifications)).await;
        assert!(
            turn.get("error").is_none(),
            "the resumed session stays usable: {turn}"
        );
        assert_eq!(turn["result"]["text"], FINISHED, "{turn}");
        assert_eq!(
            run_terminals(&notifications, &original),
            vec![Ok(FINISHED.to_owned())],
            "the original's resumed run completes under its controller grant"
        );
        let changes = tool_config_changes(&notifications, &original);
        if revoke_while_down {
            // The agent's selection for the server resolves the revoked
            // lineage: the native attachment conjunct denies exactly that
            // server's restore, once, and nothing re-stages or launches it.
            assert_eq!(
                restore_refusals(&changes),
                vec![(installed.name.clone(), "denied".to_owned())],
                "the typed governed refusal is reported once: {changes:?}"
            );
            assert!(
                !restaged_add(&changes, &installed.name),
                "the refused attachment is not re-staged: {changes:?}"
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(!marker.exists(), "the refused installation never starts");
        } else {
            assert!(
                restore_refusals(&changes).is_empty(),
                "nothing is refused: {changes:?}"
            );
            assert!(
                restaged_add(&changes, &installed.name),
                "the original's resumed turn re-stages its restored live attachment: {changes:?}"
            );
        }
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(90), second))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_governed_attachment_is_restored_after_a_host_restart() {
    stock_governed_restart(false, false).await;
}

#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_governed_attachment_revoked_while_down_is_refused_after_a_restart() {
    stock_governed_restart(true, false).await;
}

/// The first stock host's close releases everything that keeps the realm's
/// execution custody, and a new host opens the same realm root at once, with
/// no Busy. The restart's release only: the new host serves nothing, so this
/// holds without the resumed-session recovery the two restart tests need.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_governed_restart_releases_the_first_host_before_reopen() {
    stock_governed_restart(false, true).await;
}

/// The pipe's next response to `id`, recording every consent notice the
/// connection delivers before it.
#[cfg(feature = "mcp")]
async fn response_with_consent_notices(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
    id: u64,
    notices: &mut Vec<Value>,
) -> Value {
    loop {
        let mut line = String::new();
        assert!(
            reader.read_line(&mut line).await.unwrap() > 0,
            "server remains connected"
        );
        let value: Value = serde_json::from_str(&line).unwrap();
        if value["method"] == super::stock_consent::CONSENT_NOTICE_NOTIFICATION {
            notices.push(value["params"].clone());
        } else if value["id"] == id {
            return value;
        }
    }
}

/// Read the pipe until a consent notice `matches`, recording every notice.
#[cfg(feature = "mcp")]
async fn wait_for_consent_notice(
    reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
    notices: &mut Vec<Value>,
    matches: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(notice) = notices.iter().find(|notice| matches(notice)) {
                return notice.clone();
            }
            let mut line = String::new();
            assert!(
                reader.read_line(&mut line).await.unwrap() > 0,
                "server remains connected"
            );
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["method"] == super::stock_consent::CONSENT_NOTICE_NOTIFICATION {
                notices.push(value["params"].clone());
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the consent notice arrives: {notices:?}"))
}

/// The account the granted R3 server's owners state, in the RPC R3
/// scenarios.
#[cfg(feature = "mcp")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum R3Account {
    /// The host's configuration declares that the server acts with no
    /// account.
    DeclaredNone,
    /// No declaration and no connector binding: no account evidence.
    Undeclared,
    /// A connector binding selecting the issuer-verified account.
    Connector,
}

#[cfg(feature = "mcp")]
const R3_REALM: &str = "stock-consent";

/// The protocol-capable granted installation of the R3 scenarios: it records
/// every `tools/call` it receives in `calls`, and states `account`.
#[cfg(feature = "mcp")]
fn r3_granted_server(
    calls: &std::path::Path,
    account: R3Account,
    issuer: Option<&str>,
) -> meerkat_core::McpServerConfig {
    let mut granted = meerkat_core::McpServerConfig::stdio(
        "granted",
        "/bin/sh",
        vec![
            "-c".into(),
            format!(
                r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-03-26","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"granted","version":"0"}}}}}}\n' "$id" ;;
    *'"method":"tools/list"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"tools":[{{"name":"read","inputSchema":{{"type":"object"}}}}]}}}}\n' "$id" ;;
    *'"method":"tools/call"'*) printf '%s\n' "$line" >> '{calls}'; printf '{{"jsonrpc":"2.0","id":%s,"result":{{"content":[{{"type":"text","text":"granted-read"}}]}}}}\n' "$id" ;;
  esac
done
"#,
                calls = calls.display(),
            ),
        ],
        HashMap::new(),
    );
    granted.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
    match account {
        R3Account::DeclaredNone => {
            granted.account = Some(meerkat_core::McpAccountDeclaration::NoAccount);
        }
        R3Account::Undeclared => {}
        R3Account::Connector => {
            granted.connector = Some(meerkat_core::mcp_config::McpConnectorBinding {
                realm_id: R3_REALM.into(),
                slot_id: "mail".into(),
                issuer: issuer.expect("a connector binding names its issuer").into(),
                client: "client".into(),
                resource: "https://api.invalid".into(),
                strategy_id: "oidc-userinfo-v1".into(),
                account: meerkat::test_fixtures::mcp_oauth::SUBJECT.into(),
                operations: [(
                    "read".to_string(),
                    meerkat_core::mcp_config::McpConnectorOperation {
                        required_scopes: ["openid".to_string()].into(),
                        destination_origin: "https://api.invalid".into(),
                        read_only: true,
                    },
                )]
                .into(),
            });
        }
    }
    granted
}

/// A stock-governed realm whose owner requires R3 for the granted server's
/// `read`, accepting `assurance`, with the agent granted its controller's
/// route and that operation.
#[cfg(feature = "mcp")]
struct R3Realm {
    root: tempfile::TempDir,
    realm: RealmId,
    principals: meerkat::governed_realm::stock::StockPrincipals,
    granted: meerkat_core::McpServerConfig,
    calls: std::path::PathBuf,
    _markers: tempfile::TempDir,
    issuer: Option<meerkat::test_fixtures::mcp_oauth::McpOAuthFixture>,
    /// The assurance the owner's R3 rule accepts.
    assurance: String,
    /// The controller's model client: the agent is granted exactly its
    /// route, so every host on the realm reuses it (each fixture client
    /// carries its own auth binding, and with it its own route).
    client: Arc<dyn LlmClient>,
    /// The grant the agent's selection names for the granted server.
    installation_grant: Option<GrantLineageRef>,
}

/// One running stock host of an [`R3Realm`]: its pipe and the session it
/// created and promoted.
#[cfg(feature = "mcp")]
struct R3Host {
    r: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    w: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    /// The pipe's spawned serving future; `None` once it was dropped.
    server_task: Option<tokio::task::JoinHandle<Result<(), crate::server::ServerError>>>,
    runtime: Arc<SessionRuntime>,
    issuer: meerkat_core::ApprovalService,
    /// The pipe's consent surface, and the composition it decides for.
    consent: Arc<super::stock_consent::StdioConsentSurface>,
    governance: Arc<meerkat::governed_realm::stock::StockGovernance>,
    sid: String,
    notices: Vec<Value>,
    /// What the pipe must present for a requested approval: the assurance
    /// the rule accepts and the account the installation's owners state.
    expected_assurance: String,
    expected_account: Option<String>,
}

#[cfg(feature = "mcp")]
impl R3Realm {
    async fn new(http: &Server, account: R3Account, assurance: &str) -> Self {
        use meerkat::governed_realm::RealmGovernance;
        use meerkat::governed_realm::stock::issuance::{
            DEFAULT_STOCK_AGENT, McpOperations, StockGrantRequest, StockGrantTarget,
            grant_stock_agent,
        };
        use meerkat::governed_realm::stock::{StockPrincipals, govern_stock_realm};
        let markers = tempfile::tempdir().unwrap();
        let calls = markers.path().join("granted-calls.log");
        // The connector's issuer: a real OAuth/OIDC fixture the host's
        // connector authority logs in against.
        let issuer = match account {
            R3Account::Connector => Some(
                meerkat::test_fixtures::mcp_oauth::McpOAuthFixture::spawn()
                    .await
                    .unwrap(),
            ),
            R3Account::DeclaredNone | R3Account::Undeclared => None,
        };
        let granted =
            r3_granted_server(&calls, account, issuer.as_ref().map(|issuer| issuer.base()));
        let realm = RealmId::parse(R3_REALM).unwrap();
        let mut this = Self {
            root: tempfile::tempdir().unwrap(),
            principals: StockPrincipals::for_user(&realm, 4243).unwrap(),
            realm,
            granted,
            calls,
            _markers: markers,
            issuer,
            assurance: assurance.to_owned(),
            client: http_client_in(http, R3_REALM),
            installation_grant: None,
        };
        drop(this.open(RealmGovernance::Ungoverned).await.unwrap());
        meerkat_core::McpRealmPersistTarget::new(&this.config_runtime())
            .add(this.granted.clone(), None)
            .await
            .expect("realm installation");
        // The trusted host administers the realm's review rules in its own
        // document; an ordinary config write could not.
        let path = meerkat_store::realm_paths_in(this.root.path(), R3_REALM).config_path;
        let mut document = std::fs::read_to_string(&path).unwrap();
        write!(
            document,
            "\n[[governance.review.rules]]\n\
             target = {{ kind = \"mcp_operation\", server = \"granted\", operation = \"read\" }}\n\
             tier = \"r3\"\n\
             consent_assurance = \"{assurance}\"\n"
        )
        .unwrap();
        std::fs::write(&path, &document).unwrap();
        govern_stock_realm(this.root.path(), R3_REALM, &this.principals)
            .await
            .expect("governed");
        let selected = this.client.controller_model_selection().unwrap();
        let pin = {
            use meerkat_core::AgentLlmClient as _;
            Arc::new(meerkat_llm_core::LlmClientAdapter::new(
                Arc::clone(&this.client),
                selected.model().to_string(),
            ))
            .pin_controller()
            .expect("the controller pin")
        };
        let route = meerkat::governed_realm::stock::controller::controller_route_key(&pin)
            .expect("the controller's actual route");
        for target in [
            StockGrantTarget::Controller {
                routes: [route].into(),
            },
            StockGrantTarget::Mcp {
                server: this.granted.name.clone(),
                operations: McpOperations::Exact(["read".to_string()].into()),
            },
        ] {
            let issued = grant_stock_agent(
                this.root.path(),
                R3_REALM,
                &this.principals,
                &StockGrantRequest {
                    agent: DEFAULT_STOCK_AGENT.into(),
                    target,
                    expires_in_ms: None,
                },
            )
            .await
            .expect("issued and selected")
            .issued;
            // The last grant is the granted server's.
            this.installation_grant = Some(issued);
        }
        this
    }

    async fn open(
        &self,
        governance: meerkat::governed_realm::RealmGovernance,
    ) -> Result<
        (meerkat_store::RealmManifestPin, meerkat::PersistenceBundle),
        meerkat::PersistenceError,
    > {
        meerkat::governed_realm::open_governed_realm_persistence_with_provider(
            &meerkat::DiskStorageProvider,
            self.root.path(),
            R3_REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
            governance,
        )
        .await
    }

    fn config_runtime(&self) -> meerkat_core::ConfigRuntime {
        let paths = meerkat_store::realm_paths_in(self.root.path(), R3_REALM);
        meerkat_core::ConfigRuntime::new(
            Arc::new(meerkat_core::FileConfigStore::new(
                paths.config_path.clone(),
                meerkat_models::canonical(),
            )),
            paths.config_path.with_file_name("config_state.json"),
        )
    }

    /// The trusted host replaces the realm's granted installation with
    /// `granted`: the same server name, another binding.
    async fn replace_installation(&mut self, granted: meerkat_core::McpServerConfig) {
        let realm_config = self.config_runtime();
        let persist = meerkat_core::McpRealmPersistTarget::new(&realm_config);
        persist
            .remove(&self.granted.name, None)
            .await
            .expect("the old binding is removed");
        persist
            .add(granted.clone(), None)
            .await
            .expect("the new binding is installed");
        self.granted = granted;
    }

    /// Start one stock host on the realm, create a deferred session over its
    /// pipe and promote it with one governed turn.
    async fn start_host(&self, http: &Server) -> R3Host {
        let (setup, governance, issuer) = self.reviewed_setup(http).await;
        let consent = setup.consent.clone().expect("the pipe's consent surface");
        http.receiver.direct_answer.store(true, Ordering::SeqCst);
        let (client_io, server_io) = tokio::io::duplex(1 << 20);
        let (r, w) = tokio::io::split(server_io);
        // The pipe's own serving future, as the stdio host serves it, owning
        // the pipe's consent retirement from its creation.
        let retirement = consent_retirement(&setup);
        let (serving, runtime) =
            governed_serving(BufReader::new(r), w, setup, None, retirement).expect("construct");
        let server_task = tokio::spawn(serving);
        let (r, w) = tokio::io::split(client_io);
        // The granted installation's owners state its connector binding's
        // account, its declared absence, or nothing.
        let expected_account = self
            .granted
            .connector
            .as_ref()
            .map(|binding| format!("{} at {}", binding.account, binding.issuer));
        R3Host {
            r: BufReader::new(r),
            w,
            server_task: Some(server_task),
            runtime,
            issuer,
            consent,
            governance,
            sid: String::new(),
            notices: Vec::new(),
            expected_assurance: self.assurance.clone(),
            expected_account,
        }
    }

    /// The reviewed stock setup of one host on the realm, with the stock
    /// composition it serves and the persistence's one approval issuer.
    async fn reviewed_setup(
        &self,
        http: &Server,
    ) -> (
        GovernedJsonlSetup,
        Arc<meerkat::governed_realm::stock::StockGovernance>,
        meerkat_core::ApprovalService,
    ) {
        use meerkat::governed_realm::RealmGovernance;
        use meerkat::governed_realm::stock::StockGovernance;
        let governance = StockGovernance::new(self.realm.clone(), self.principals.clone());
        let (_, bundle) = self
            .open(RealmGovernance::Stock(Arc::clone(&governance)))
            .await
            .unwrap();
        let issuer = bundle.approval_issuer();
        // The client whose route the agent was granted.
        let client = Arc::clone(&self.client);
        let selected = client.controller_model_selection().unwrap();
        let (mut config, _) = governed_client_config(http, &selected, &bundle).await;
        config.tools.mcp_servers = vec![self.granted.clone()];
        // The host's effective config: the same document the composition
        // read.
        let document = std::fs::read_to_string(
            meerkat_store::realm_paths_in(self.root.path(), R3_REALM).config_path,
        )
        .unwrap();
        config.governance = Config::from_persisted_toml(&document)
            .expect("the realm's document")
            .0
            .governance;
        let connector_authority = match (&self.issuer, &self.granted.connector) {
            (Some(issuer), Some(binding)) => {
                let authority = meerkat::HostAuthService::new(
                    meerkat::ProviderAuthPersistence::new(
                        Arc::new(meerkat_providers::auth_store::EphemeralTokenStore::new()),
                        Arc::new(meerkat_providers::auth_store::InMemoryCoordinator::new()),
                    ),
                    bundle.runtime_adapter().provider_auth_runtime_authority(),
                )
                .connector_oauth_authority()
                .expect("the host's connector authority");
                // The connector account is authorized: one real login against
                // the issuer for the installation's own slot, account and
                // scopes.
                let slot = meerkat_core::connection::CredentialAccountRef {
                    realm: self.realm.clone(),
                    account: meerkat_core::connection::CredentialAccountId::parse(&binding.slot_id)
                        .unwrap(),
                };
                let redirect = "http://127.0.0.1:1/connector/oauth/callback";
                let started = authority
                    .login_start(
                        &meerkat::ConnectorOAuthTarget {
                            slot: slot.clone(),
                            issuer: issuer.base().into(),
                            client: binding.client.clone(),
                            resource: binding.resource.clone(),
                            scopes: ["openid".to_string()].into(),
                            strategy_id: binding.strategy_id.clone(),
                            account: meerkat_providers::connector_oauth::AccountSelection::Known(
                                binding.account.clone(),
                            ),
                            client_auth_profile: None,
                        },
                        redirect,
                    )
                    .await
                    .expect("connector login starts");
                authority
                    .login_complete(
                        &slot,
                        meerkat::ConnectorOAuthCallback {
                            redirect_uri: redirect.into(),
                            state: started.state,
                            code: meerkat::test_fixtures::mcp_oauth::CODE_CANARY.into(),
                            host_attempt_handle: None,
                        },
                    )
                    .await
                    .expect("the connector account is authorized");
                Some(authority)
            }
            _ => None,
        };
        let setup = stock::stock_governed_jsonl_setup_reviewed(
            &governance,
            config,
            bundle,
            client,
            Vec::new(),
            connector_authority,
            &meerkat::AgentFactory::minimal(),
        )
        .await
        .expect("the reviewed stock setup");
        assert!(setup.operation_review.is_some() && setup.consent.is_some());
        (setup, governance, issuer)
    }
}

#[cfg(feature = "mcp")]
impl R3Host {
    async fn response(&mut self, id: u64) -> Value {
        response_with_consent_notices(&mut self.r, id, &mut self.notices).await
    }

    /// Initialize the pipe, create a deferred session over it and promote it
    /// with one governed turn.
    async fn begin_session(&mut self) {
        send(
            &mut self.w,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        )
        .await;
        let initialized = self.response(1).await;
        assert!(
            initialized["result"]["methods"]
                .as_array()
                .unwrap()
                .iter()
                .any(|method| method == crate::router::CONSENT_DECIDE_METHOD),
            "{initialized}"
        );
        send(
            &mut self.w,
            json!({"jsonrpc":"2.0","id":2,"method":"session/create","params":{
                "prompt":"deferred-seed-consent", "initial_turn":"deferred"
            }}),
        )
        .await;
        let created = self.response(2).await;
        assert!(created.get("error").is_none(), "{created}");
        self.sid = created["result"]["session_id"].as_str().unwrap().to_owned();
        send(
            &mut self.w,
            json!({"jsonrpc":"2.0","id":3,"method":"turn/start","params":{
                "session_id":self.sid, "prompt":"first"
            }}),
        )
        .await;
        let first = self.response(3).await;
        assert!(first.get("error").is_none(), "{first}");
        settle_mcp(&self.runtime, &SessionId::parse(&self.sid).unwrap()).await;
    }

    /// The model calls the granted R3 operation in a new turn; returns the
    /// turn's response.
    async fn call_read(&mut self, http: &Server, id: u64) -> Value {
        *http.receiver.tool_call.lock().unwrap() = Some("read".into());
        send(
            &mut self.w,
            json!({"jsonrpc":"2.0","id":id,"method":"turn/start","params":{
                "session_id":self.sid, "prompt":"call read"
            }}),
        )
        .await;
        let called = self.response(id).await;
        assert!(called.get("error").is_none(), "{called}");
        called
    }

    /// The pipe's `requested` notice: it presents, before any decision,
    /// exactly what the one issuer recorded. The record read here is only
    /// the oracle; every presented fact is asserted on the wire.
    async fn requested(&mut self) -> (String, meerkat_core::ApprovalRecord) {
        use meerkat::governed_realm::stock::consent::{
            STOCK_ACTION_ACCOUNT_LABEL, STOCK_REVIEW_MANDATE_LABEL,
        };
        let requested = wait_for_consent_notice(&mut self.r, &mut self.notices, |notice| {
            notice["kind"] == "requested"
        })
        .await;
        let approval_id = requested["approval_id"].as_str().unwrap().to_owned();
        let record = self
            .issuer
            .get(&approval_id.parse().unwrap())
            .expect("the one issuer holds the approval");
        let presented = &requested["presentation"];
        // The exact action: the granted operation, with the call's arguments.
        assert_eq!(
            presented["action"],
            json!({"kind": "tool_call", "summary": "read", "body": {}}),
            "{requested}"
        );
        assert_eq!(
            presented["action"],
            serde_json::to_value(&record.proposed_action).unwrap()
        );
        assert_eq!(
            presented["risk"],
            serde_json::to_value(record.risk).unwrap()
        );
        assert_eq!(
            presented["accepted_assurance"],
            self.expected_assurance.as_str()
        );
        // The account the installation's owners state, absent when they
        // declared none.
        assert_eq!(
            presented["account"].as_str(),
            self.expected_account.as_deref(),
            "{requested}"
        );
        assert_eq!(
            presented["account"].as_str(),
            record
                .metadata
                .labels
                .get(STOCK_ACTION_ACCOUNT_LABEL)
                .map(String::as_str)
        );
        // The stock mandate of the operator's own invocation.
        let mandate = presented["mandate"]
            .as_str()
            .expect("the presented mandate");
        assert!(
            mandate.starts_with("The local operator ")
                && mandate.contains(&format!("in realm {R3_REALM}"))
                && mandate.contains("under a grant-lineage mandate")
                && mandate.contains("bounds this operation to"),
            "{mandate}"
        );
        assert_eq!(
            Some(mandate),
            record
                .metadata
                .labels
                .get(STOCK_REVIEW_MANDATE_LABEL)
                .map(String::as_str)
        );
        let expires_at: chrono::DateTime<chrono::Utc> =
            serde_json::from_value(presented["expires_at"].clone()).expect("the expiry");
        assert_eq!(Some(expires_at), record.expires_at);
        assert!(
            expires_at > chrono::Utc::now(),
            "the approval has not expired"
        );
        (approval_id, record)
    }

    async fn decide(&mut self, id: u64, approval_id: &str) -> Value {
        send(
            &mut self.w,
            json!({"jsonrpc":"2.0","id":id,"method":"approval/consent_decide","params":{
                "approval_id": approval_id, "decision": "approve"
            }}),
        )
        .await;
        self.response(id).await
    }

    /// Drop the pipe's serving future before its connection reaches the
    /// close boundary, as when the task that owns it is aborted, and join it
    /// within its bound.
    async fn drop_serving(&mut self) {
        let outcome = {
            let task = self.server_task.as_mut().expect("the pipe is served");
            task.abort();
            tokio::time::timeout(Duration::from_secs(5), task).await
        };
        // Only a completed handle leaves the host: past the bound, or when
        // this wait is itself cancelled, `close` still joins it.
        if outcome.is_ok() {
            self.server_task = None;
        }
        match outcome {
            Ok(Err(error)) if error.is_cancelled() => {}
            other => panic!("the serving future is dropped, not completed: {other:?}"),
        }
    }

    /// Eligibility of this pipe's own operator to decide `approval`,
    /// observed on the composition's publication as the stock consent route
    /// observes it; `None` when the pipe vouches for no such login.
    fn pipe_eligibility(
        &self,
        approval: &meerkat_core::ApprovalRecord,
    ) -> Option<Arc<dyn meerkat_core::approval::consent::ConsentEligibility>> {
        use meerkat::governed_realm::stock::consent::StockConsentSurface as _;
        let approver = self.consent.approver().expect("the pipe's approver");
        meerkat_authorization::publication::PublishedConsentEligibility::observe(
            &self.governance.authorization_publication(),
            || self.consent.approver_login(&approver, approval).is_some(),
        )
        .ok()
    }

    /// The pipe's own approver (its operator, device trusted, in its scope)
    /// approves `approval_id` directly on the native owner.
    fn decide_in_pipe_scope(
        &self,
        approval_id: &str,
    ) -> Result<meerkat_core::ApprovalRecord, meerkat_core::ApprovalError> {
        self.issuer.decide_consent_event(
            meerkat_core::approval::consent::AuthenticatedConsentEvent::new(
                approval_id.parse().unwrap(),
                meerkat_core::approval::consent::AuthenticatedConsentDecision {
                    approver: self.consent.approver().expect("the pipe's approver"),
                    decision: meerkat_core::ApprovalDecision::Approve,
                    reason: None,
                },
            ),
        )
    }

    /// The pipe closes and the whole host goes away, each step within its
    /// bound and the server task joined, never detached. Returns every
    /// step's failure: empty when the server ended cleanly and the runtime
    /// shut down.
    async fn close(self) -> Vec<String> {
        let Self {
            mut r,
            mut w,
            server_task,
            runtime,
            issuer,
            ..
        } = self;
        let mut failures = Vec::new();
        let mut drain = tokio::spawn(async move {
            let mut line = String::new();
            while r.read_line(&mut line).await.unwrap_or(0) > 0 {
                line.clear();
            }
        });
        match tokio::time::timeout(Duration::from_secs(2), w.shutdown()).await {
            Ok(Ok(())) => {}
            other => failures.push(format!("pipe writer shutdown: {other:?}")),
        }
        // Borrow the JoinHandle: a timeout must not detach the server task.
        if let Some(mut server_task) = server_task {
            match tokio::time::timeout(Duration::from_secs(30), &mut server_task).await {
                Ok(Ok(Ok(()))) => {}
                Ok(other) => failures.push(format!("server completion: {other:?}")),
                Err(_) => {
                    failures.push("the server did not stop within its bound".into());
                    server_task.abort();
                    match tokio::time::timeout(Duration::from_secs(2), &mut server_task).await {
                        Ok(Err(error)) if error.is_cancelled() => {}
                        Ok(Ok(Ok(()))) => {}
                        other => failures.push(format!("server abort/join: {other:?}")),
                    }
                }
            }
        }
        drain.abort();
        match tokio::time::timeout(Duration::from_secs(2), &mut drain).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) if error.is_cancelled() => {}
            other => failures.push(format!("pipe drain abort/join: {other:?}")),
        }
        match tokio::time::timeout(Duration::from_secs(30), runtime.try_shutdown()).await {
            Ok(Ok(())) => {}
            other => failures.push(format!("runtime shutdown: {other:?}")),
        }
        drop(runtime);
        drop(issuer);
        failures
    }
}

/// Run `scenario` within its bound, catching a failed assertion so that the
/// host is always closed after it.
#[cfg(feature = "mcp")]
async fn r3_bounded<T>(
    scenario: impl std::future::Future<Output = T>,
) -> std::thread::Result<Result<T, tokio::time::error::Elapsed>> {
    std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(180), scenario))
        .catch_unwind()
        .await
}

/// The tool result the model saw for the single call.
#[cfg(feature = "mcp")]
fn single_call_result(http: &Server) -> Value {
    http.receiver
        .bodies
        .lock()
        .unwrap()
        .iter()
        .flat_map(|body| body["messages"].as_array().cloned().unwrap_or_default())
        .flat_map(|message| message["content"].as_array().cloned().unwrap_or_default())
        .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == SINGLE_CALL)
        .expect("the call's tool result reached the model")
}

/// R3 consent on the stdio stock host, over an installation its host
/// declares acts with no account: the issued approval names no account, the
/// native owner issues it through the persistence's one issuer, a spoofed
/// approver is refused as an invalid parameter, and the pipe's
/// device-trusted operator decides it under a rule that accepts device trust
/// (or is refused under one that requires a person).
#[cfg(feature = "mcp")]
async fn stock_r3_declared_none_scenario(assurance: &str) {
    let mut http = Server::start().await;
    let realm = R3Realm::new(&http, R3Account::DeclaredNone, assurance).await;
    let mut host = realm.start_host(&http).await;
    let outcome = r3_bounded(async {
        host.begin_session().await;
        host.call_read(&http, 4).await;
        assert!(
            !realm.calls.exists(),
            "an R3 operation runs only after consent"
        );
        let (approval_id, pending) = host.requested().await;
        assert_eq!(pending.status, meerkat_core::ApprovalStatus::Pending);
        assert!(
            !pending
                .metadata
                .labels
                .contains_key(meerkat::governed_realm::stock::consent::STOCK_ACTION_ACCOUNT_LABEL),
            "the owner declared that no account applies"
        );

        // JSON cannot name the approver, its assurance or its scope.
        send(
            &mut host.w,
            json!({"jsonrpc":"2.0","id":5,"method":"approval/consent_decide","params":{
                "approval_id": approval_id, "decision": "approve",
                "approver": {"kind": "human", "id": "someone-else"}
            }}),
        )
        .await;
        let spoofed = host.response(5).await;
        assert_eq!(spoofed["error"]["code"], error::INVALID_PARAMS, "{spoofed}");

        let decided = host.decide(6, &approval_id).await;
        let record = host
            .issuer
            .get(&approval_id.parse().unwrap())
            .expect("the one issuer holds the approval");
        if assurance == "device_trusted" {
            // The pipe's operator is device trusted, which this rule accepts.
            assert!(decided.get("error").is_none(), "{decided}");
            assert_eq!(record.status, meerkat_core::ApprovalStatus::Approved);
            let approver = record
                .consent
                .as_ref()
                .and_then(|consent| consent.approver.as_ref())
                .expect("the recorded approver");
            assert_eq!(approver.principal, realm.principals.operator);
            assert_eq!(
                approver.assurance,
                meerkat_core::ApprovalAssurance::DeviceTrusted
            );
            // The decision's notice reaches the pipe; nothing executed.
            wait_for_consent_notice(&mut host.r, &mut host.notices, |notice| {
                notice["kind"] == "approved" && notice["approval_id"] == approval_id.as_str()
            })
            .await;
            assert!(
                !realm.calls.exists(),
                "an approval executes nothing by itself"
            );
        } else {
            // A rule that requires person authentication refuses the pipe's
            // device trust; the approval stays pending.
            assert!(decided.get("error").is_some(), "{decided}");
            assert_eq!(record.status, meerkat_core::ApprovalStatus::Pending);
        }
    })
    .await;
    let mut failures = host.close().await;
    http.join_receiver(&mut failures).await;
    finish_scenario(outcome, failures);
}

/// R3 consent on the stdio stock host is decided by the pipe's device-trusted
/// operator, for an installation declared to act with no account.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_consent_is_decided_by_the_pipes_device_trusted_operator() {
    stock_r3_declared_none_scenario("device_trusted").await;
}

/// A rule that requires person authentication refuses the pipe's device
/// trust: the decision is refused and the approval stays pending.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_consent_requiring_a_person_refuses_the_pipes_device_trust() {
    stock_r3_declared_none_scenario("person_authenticated").await;
}

/// The `read` calls the granted R3 server received.
#[cfg(feature = "mcp")]
fn granted_calls(calls: &std::path::Path) -> usize {
    std::fs::read_to_string(calls).map_or(0, |calls| calls.lines().count())
}

/// An approved R3 consent spent by the run that requested it, at that run's
/// next boundary after the decision (ADR-001 D9). The model calls the
/// granted `read`, which requests consent; the run's next model request is
/// held while the pipe's operator approves it (its approved notice reaches
/// the pipe) and `after_the_decision` runs on the host. The held answer
/// calls a tool the agent does not have, so the run reaches another
/// boundary, which spends the decided consent before its next request.
/// Returns the approval's record once the turn completed.
#[cfg(feature = "mcp")]
async fn stock_r3_spend_in_the_requesting_run(
    http: &Server,
    host: &mut R3Host,
    after_the_decision: impl FnOnce(&R3Host),
) -> meerkat_core::ApprovalRecord {
    host.begin_session().await;
    http.receiver.script.lock().unwrap().extend([
        ScriptedAnswer {
            call_id: SINGLE_CALL,
            tool: "read",
            held: false,
        },
        ScriptedAnswer {
            call_id: "toolu_unpublished",
            tool: "unpublished_tool",
            held: true,
        },
    ]);
    send(
        &mut host.w,
        json!({"jsonrpc":"2.0","id":4,"method":"turn/start","params":{
            "session_id":host.sid, "prompt":"call read"
        }}),
    )
    .await;
    let (approval_id, pending) = host.requested().await;
    assert_eq!(pending.status, meerkat_core::ApprovalStatus::Pending);
    tokio::time::timeout(
        Duration::from_secs(30),
        http.receiver.second_request.notified(),
    )
    .await
    .expect("the run's next request is held while its consent is pending");
    let decided = host
        .decide_in_pipe_scope(&approval_id)
        .expect("approved in the pipe's scope while the run is open");
    assert_eq!(decided.status, meerkat_core::ApprovalStatus::Approved);
    wait_for_consent_notice(&mut host.r, &mut host.notices, |notice| {
        notice["kind"] == "approved" && notice["approval_id"] == approval_id.as_str()
    })
    .await;
    after_the_decision(&*host);
    http.receiver.finish.notify_one();
    let finished = host.response(4).await;
    assert!(finished.get("error").is_none(), "{finished}");
    host.issuer
        .get(&approval_id.parse().unwrap())
        .expect("the approval")
}

/// ADR-001 D9 on the stdio stock host: an approved consent outlives an
/// unrelated owner change on the composition's publication between its
/// decision and its spend (as another turn, or a host's change of its
/// callers' facts, publishes one). The spend asks the route again whether
/// the pipe's operator was revoked since deciding; the pipe still answers
/// for them through the approval, so `read` enters exactly once.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_an_approved_consent_enters_once_after_an_unrelated_owner_change() {
    let mut http = Server::start().await;
    let realm = R3Realm::new(&http, R3Account::DeclaredNone, "device_trusted").await;
    let mut host = realm.start_host(&http).await;
    let outcome = r3_bounded(async {
        let record = stock_r3_spend_in_the_requesting_run(&http, &mut host, |host| {
            host.governance.apply_host_change(|| ());
        })
        .await;
        assert!(
            record.consent.as_ref().is_some_and(
                |consent| consent.consumed_at.is_some() && consent.withdrawal.is_none()
            ),
            "{record:?}"
        );
        assert_eq!(granted_calls(&realm.calls), 1, "read entered exactly once");
        wait_for_consent_notice(&mut host.r, &mut host.notices, |notice| {
            notice["kind"] == "used"
        })
        .await;
    })
    .await;
    let mut failures = host.close().await;
    http.join_receiver(&mut failures).await;
    finish_scenario(outcome, failures);
}

/// ADR-001 D9 on the stdio stock host: the pipe's retirement (its close)
/// revokes its operator. Approved, then retired before the spend, the
/// consent is withdrawn as `AuthorityChanged` at the spend, with zero entry.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_an_approved_consent_never_enters_once_its_pipe_retired() {
    let mut http = Server::start().await;
    let realm = R3Realm::new(&http, R3Account::DeclaredNone, "device_trusted").await;
    let mut host = realm.start_host(&http).await;
    let outcome = r3_bounded(async {
        let record = stock_r3_spend_in_the_requesting_run(&http, &mut host, |host| {
            host.consent.retire();
        })
        .await;
        assert_eq!(record.status, meerkat_core::ApprovalStatus::Approved);
        assert_eq!(
            record
                .consent
                .as_ref()
                .and_then(|consent| consent.withdrawal.as_ref())
                .map(|withdrawal| withdrawal.reason.clone()),
            Some(meerkat_core::approval::ApprovalConsentWithdrawalReason::AuthorityChanged),
            "{record:?}"
        );
        assert!(
            record
                .consent
                .as_ref()
                .is_some_and(|consent| consent.consumed_at.is_none()),
            "nothing spent"
        );
        assert!(!realm.calls.exists(), "zero entry");
    })
    .await;
    let mut failures = host.close().await;
    http.join_receiver(&mut failures).await;
    finish_scenario(outcome, failures);
}

/// No controller work holds the grant a test revokes live.
#[cfg(feature = "mcp")]
struct NoControllerWork;

#[cfg(feature = "mcp")]
impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
    for NoControllerWork
{
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<
        Result<T, E>,
        meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal,
    > {
        Ok(mutate())
    }
}

/// ADR-001 D9 on the stdio stock host: a policy change between the decision
/// and the spend withdraws the approved consent. The operator revokes the
/// granted server's grant on the open composition's own grant owner; the
/// spend's owners, asked afresh, refuse the operation, so the consent is
/// withdrawn as `AuthorityChanged`, with zero entry.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_an_approved_consent_never_enters_once_its_grant_is_revoked() {
    let mut http = Server::start().await;
    let realm = R3Realm::new(&http, R3Account::DeclaredNone, "device_trusted").await;
    let grant = realm
        .installation_grant
        .clone()
        .expect("the granted server's grant");
    let operator = realm.principals.operator.clone();
    let mut host = realm.start_host(&http).await;
    let outcome = r3_bounded(async {
        let record = stock_r3_spend_in_the_requesting_run(&http, &mut host, |host| {
            let grants = host
                .governance
                .composed_grant_owner_for_test()
                .expect("the open composition's grant owner");
            // The revocation never waits: it answers `Unavailable` while
            // another operation holds the owner's lock or its publication
            // (the host's own traffic). Ask again until it is free, within
            // a few seconds, without holding the publication meanwhile.
            tokio::task::block_in_place(|| {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                loop {
                    match grants.revoke(&operator, &grant, &mut NoControllerWork) {
                        Ok(()) => break,
                        Err(meerkat_authorization::grants::GrantRefusal::Unavailable)
                            if std::time::Instant::now() < deadline =>
                        {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(refusal) => panic!("revoked while the realm is open: {refusal}"),
                    }
                }
            });
        })
        .await;
        assert_eq!(record.status, meerkat_core::ApprovalStatus::Approved);
        assert_eq!(
            record
                .consent
                .as_ref()
                .and_then(|consent| consent.withdrawal.as_ref())
                .map(|withdrawal| withdrawal.reason.clone()),
            Some(meerkat_core::approval::ApprovalConsentWithdrawalReason::AuthorityChanged),
            "{record:?}"
        );
        assert!(
            record
                .consent
                .as_ref()
                .is_some_and(|consent| consent.consumed_at.is_none()),
            "nothing spent"
        );
        assert!(!realm.calls.exists(), "zero entry");
    })
    .await;
    let mut failures = host.close().await;
    http.join_receiver(&mut failures).await;
    finish_scenario(outcome, failures);
}

/// A connector-bound installation states its binding's issuer-verified
/// account: the issued approval names it, and the pipe's operator decides
/// it. Nothing executes on the decision.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_consent_names_the_connector_bound_account() {
    let mut http = Server::start().await;
    let realm = R3Realm::new(&http, R3Account::Connector, "device_trusted").await;
    let mut host = realm.start_host(&http).await;
    let outcome = r3_bounded(async {
        host.begin_session().await;
        host.call_read(&http, 4).await;
        let (approval_id, pending) = host.requested().await;
        assert_eq!(pending.status, meerkat_core::ApprovalStatus::Pending);
        let issuer = realm
            .issuer
            .as_ref()
            .expect("the connector's issuer")
            .base();
        assert_eq!(
            pending
                .metadata
                .labels
                .get(meerkat::governed_realm::stock::consent::STOCK_ACTION_ACCOUNT_LABEL)
                .map(String::as_str),
            Some(format!("{} at {issuer}", meerkat::test_fixtures::mcp_oauth::SUBJECT).as_str()),
            "the approval names the binding's account"
        );
        let decided = host.decide(5, &approval_id).await;
        assert!(decided.get("error").is_none(), "{decided}");
        assert_eq!(
            host.issuer
                .get(&approval_id.parse().unwrap())
                .expect("the approval")
                .status,
            meerkat_core::ApprovalStatus::Approved
        );
        assert!(
            !realm.calls.exists(),
            "an approval executes nothing by itself"
        );
    })
    .await;
    let mut failures = host.close().await;
    http.join_receiver(&mut failures).await;
    finish_scenario(outcome, failures);
}

/// An installation whose owners state no account (no declaration and no
/// connector binding) has no account evidence: its R3 operation's review
/// material is unavailable, so no approval is issued, nothing is presented
/// to the pipe and nothing executes; the model sees a refusal.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_without_account_evidence_requests_no_consent() {
    let mut http = Server::start().await;
    let realm = R3Realm::new(&http, R3Account::Undeclared, "device_trusted").await;
    let mut host = realm.start_host(&http).await;
    let outcome = r3_bounded(async {
        host.begin_session().await;
        let called = host.call_read(&http, 4).await;
        assert_eq!(called["result"]["text"], FINISHED, "{called}");
        assert_eq!(
            single_call_result(&http)["is_error"],
            json!(true),
            "the model sees the refusal"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            host.notices.is_empty(),
            "nothing is presented: {:?}",
            host.notices
        );
        assert!(
            host.issuer
                .list(meerkat_core::ApprovalListFilter::default())
                .expect("the issuer lists")
                .is_empty(),
            "no approval is issued"
        );
        assert!(!realm.calls.exists(), "nothing executes");
    })
    .await;
    let mut failures = host.close().await;
    http.join_receiver(&mut failures).await;
    finish_scenario(outcome, failures);
}

/// A changed binding carries nothing over. Consent is requested for the
/// installation declared to act with no account; the trusted host then
/// replaces the realm's installation with one whose owners state no account
/// and restarts. The new host reviews the same operation on the new binding
/// alone: no approval is issued for it, and the approval requested on the
/// old binding cannot be decided on the new host.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_a_changed_binding_carries_no_consent_over() {
    let mut http = Server::start().await;
    let mut realm = R3Realm::new(&http, R3Account::DeclaredNone, "device_trusted").await;
    let mut host = realm.start_host(&http).await;
    let first = r3_bounded(async {
        host.begin_session().await;
        host.call_read(&http, 4).await;
        let (old_approval, pending) = host.requested().await;
        assert_eq!(pending.status, meerkat_core::ApprovalStatus::Pending);
        old_approval
    })
    .await;
    let mut failures = host.close().await;
    let old_approval = match first {
        Ok(Ok(old_approval)) if failures.is_empty() => old_approval,
        first => {
            http.join_receiver(&mut failures).await;
            match first {
                Err(panic) => std::panic::resume_unwind(panic),
                Ok(Err(timeout)) => {
                    panic!("the first host exceeded its bound: {timeout}; {failures:?}")
                }
                Ok(Ok(_)) => panic!("the first host did not close cleanly: {failures:?}"),
            }
        }
    };

    let replacement = r3_granted_server(&realm.calls, R3Account::Undeclared, None);
    realm.replace_installation(replacement).await;
    let mut host = realm.start_host(&http).await;
    let outcome = r3_bounded(async {
        host.begin_session().await;
        host.call_read(&http, 4).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !host
                .notices
                .iter()
                .any(|notice| notice["kind"] == "requested"),
            "the new binding states no account: {:?}",
            host.notices
        );
        let decided = host.decide(5, &old_approval).await;
        assert!(
            decided.get("error").is_some(),
            "the old binding's approval is not this host's to decide: {decided}"
        );
        assert_ne!(
            host.issuer
                .get(&old_approval.parse().unwrap())
                .expect("the old approval's record")
                .status,
            meerkat_core::ApprovalStatus::Approved
        );
        assert!(!realm.calls.exists(), "nothing executes");
    })
    .await;
    let mut failures = host.close().await;
    http.join_receiver(&mut failures).await;
    finish_scenario(outcome, failures);
}

/// The pipe's consent scope retires at the connection's close boundary,
/// before the connection drains. A turn is still running when the pipe
/// reaches EOF (the model holds its request), so the server drains it before
/// it returns: during that drain, eligibility observed for the pipe's
/// operator is no longer current and a decision in the pipe's scope is
/// refused.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_the_pipes_consent_retires_at_eof_before_the_drain() {
    let mut http = Server::start().await;
    let realm = R3Realm::new(&http, R3Account::DeclaredNone, "device_trusted").await;
    let mut host = realm.start_host(&http).await;
    let outcome = r3_bounded(async {
        host.begin_session().await;
        host.call_read(&http, 4).await;
        let (approval_id, pending) = host.requested().await;
        http.receiver.hold_next.store(true, Ordering::SeqCst);
        send(
            &mut host.w,
            json!({"jsonrpc":"2.0","id":5,"method":"turn/start","params":{
                "session_id":host.sid, "prompt":"held"
            }}),
        )
        .await;
        tokio::time::timeout(
            Duration::from_secs(30),
            http.receiver.second_request.notified(),
        )
        .await
        .expect("the held turn reaches the model");
        let observed = host
            .pipe_eligibility(&pending)
            .expect("the open pipe's operator is eligible");
        host.w.shutdown().await.unwrap();
        // The server cannot return before the held turn drains (the
        // executor's shutdown grace alone is 5 s): the fence comes first.
        let fenced = tokio::time::timeout(Duration::from_secs(4), async {
            while observed.check_current().is_ok() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            fenced.is_ok(),
            "eligibility observed for the pipe stays current after EOF"
        );
        assert!(
            host.pipe_eligibility(&pending).is_none(),
            "the closed pipe vouches for no login"
        );
        let refused = host.decide_in_pipe_scope(&approval_id);
        assert!(
            matches!(
                refused,
                Err(meerkat_core::ApprovalError::NotEligible { .. })
            ),
            "{refused:?}"
        );
        assert!(
            !host
                .server_task
                .as_ref()
                .expect("the pipe is served")
                .is_finished(),
            "the drain still holds the server"
        );
    })
    .await;
    http.receiver.finish.notify_one();
    let mut failures = host.close().await;
    http.join_receiver(&mut failures).await;
    finish_scenario(outcome, failures);
}

/// Dropping the pipe's serving future before its connection reaches the
/// close boundary (the task that owns it is aborted) still retires the
/// pipe's consent scope: eligibility observed for the pipe's operator is no
/// longer current, a decision in the pipe's scope is refused and the
/// approval stays pending.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_dropping_the_serving_future_retires_the_pipes_consent() {
    let mut http = Server::start().await;
    let realm = R3Realm::new(&http, R3Account::DeclaredNone, "device_trusted").await;
    let mut host = realm.start_host(&http).await;
    let outcome = r3_bounded(async {
        host.begin_session().await;
        host.call_read(&http, 4).await;
        let (approval_id, pending) = host.requested().await;
        let observed = host
            .pipe_eligibility(&pending)
            .expect("the open pipe's operator is eligible");
        host.drop_serving().await;
        assert!(
            observed.check_current().is_err(),
            "eligibility observed for a dropped pipe stays current"
        );
        assert!(
            host.pipe_eligibility(&pending).is_none(),
            "the dropped pipe vouches for no login"
        );
        let refused = host.decide_in_pipe_scope(&approval_id);
        assert!(
            matches!(
                refused,
                Err(meerkat_core::ApprovalError::NotEligible { .. })
            ),
            "{refused:?}"
        );
        assert_eq!(
            host.issuer
                .get(&approval_id.parse().unwrap())
                .expect("the approval")
                .status,
            meerkat_core::ApprovalStatus::Pending
        );
    })
    .await;
    let mut failures = host.close().await;
    http.join_receiver(&mut failures).await;
    finish_scenario(outcome, failures);
}

/// The public serving future owns the pipe's consent retirement from its
/// creation: created and dropped without ever being polled, it still
/// retires the pipe's scope, so eligibility observed for the pipe's operator
/// before is no longer current and a fresh observation is refused.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_r3_an_unpolled_public_serving_future_retires_the_pipes_consent() {
    use meerkat::governed_realm::stock::consent::{
        STOCK_CONSENT_ASSURANCE_LABEL, STOCK_REVIEW_MANDATE_LABEL, StockConsentSurface as _,
    };
    use meerkat_authorization::publication::PublishedConsentEligibility;
    use meerkat_core::approval::consent::{ConsentNotice, ConsentNoticeKind};
    let mut http = Server::start().await;
    let realm = R3Realm::new(&http, R3Account::DeclaredNone, "device_trusted").await;
    let outcome = r3_bounded(async {
        let (setup, governance, issuer) = realm.reviewed_setup(&http).await;
        let consent = setup.consent.clone().expect("the pipe's consent surface");

        // A pending approval, as the stock route issues it, presented on the
        // pipe: its operator is eligible to decide it.
        let mut metadata = meerkat_core::SurfaceMetadata::default();
        metadata.labels.insert(
            STOCK_CONSENT_ASSURANCE_LABEL.to_owned(),
            "device_trusted".to_owned(),
        );
        metadata.labels.insert(
            STOCK_REVIEW_MANDATE_LABEL.to_owned(),
            "the work's mandate".to_owned(),
        );
        let approval = issuer
            .request(meerkat_core::ApprovalRequest {
                requester: meerkat_core::ApprovalPrincipalId::new("human:operator").unwrap(),
                owner: meerkat_core::ApprovalOwnerRef::ToolCall {
                    tool_call_id: meerkat_core::ToolCallId::new("call-1"),
                },
                resource: meerkat_core::ApprovalResourceRef {
                    kind: meerkat_core::ApprovalResourceKind::ToolCall,
                    id: meerkat_core::ApprovalResourceId::new("read"),
                },
                proposed_action: meerkat_core::ApprovalProposedAction {
                    kind: meerkat_core::ApprovalActionKind::ToolCall,
                    summary: "read".to_owned(),
                    body: Some(json!({})),
                },
                risk: meerkat_core::ApprovalRisk::High,
                request_body: None,
                allowed_decisions: [
                    meerkat_core::ApprovalDecision::Approve,
                    meerkat_core::ApprovalDecision::Deny,
                ]
                .into_iter()
                .collect(),
                expires_at: Some(
                    chrono::Utc::now()
                        + chrono::Duration::from_std(Duration::from_secs(600)).unwrap(),
                ),
                metadata,
                request_provenance: None,
            })
            .expect("the approval");
        let (tx, mut shown) = tokio::sync::mpsc::channel(8);
        consent.attach_notices(&crate::router::NotificationSink::new(tx));
        consent.notice(ConsentNotice {
            approval_id: approval.approval_id.clone(),
            tool_call_id: "call-1".into(),
            kind: ConsentNoticeKind::Requested,
        });
        assert!(shown.try_recv().is_ok(), "the approval is presented");
        let approver = consent.approver().expect("the pipe's approver");
        let eligible = || {
            PublishedConsentEligibility::observe(&governance.authorization_publication(), || {
                consent.approver_login(&approver, &approval).is_some()
            })
        };
        let observed = eligible().expect("the open pipe's operator is eligible");

        let (_client_io, server_io) = tokio::io::duplex(1 << 10);
        let (r, w) = tokio::io::split(server_io);
        let serving = serve_governed_jsonl(BufReader::new(r), w, setup);
        drop(serving);
        assert!(
            observed.check_current().is_err(),
            "eligibility observed for the pipe stays current after its serving future is dropped"
        );
        assert!(eligible().is_err(), "the dropped pipe vouches for no login");
    })
    .await;
    // A failed assertion above still joins the loopback model server.
    let mut failures = Vec::new();
    http.join_receiver(&mut failures).await;
    finish_scenario(outcome, failures);
}

/// A stock host restarts with no live registration for a durable session:
/// a plain governed `turn/start` on the new host recovers the session
/// through the owned no-turn materialization, with the stock destination and
/// the agent bound on the actual new registration before its agent is built
/// (nothing is pre-registered by the test). The turn completes, a selected
/// but unconfigured installation then attaches live for the recovered
/// session's agent, and an unselected one is refused.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_governed_cold_turn_recovers_and_binds_the_new_registration() {
    stock_governed_cold_turn(true).await;
}

/// The control: the session's durable configured selection (binding A)
/// differs from the new host's default controller (binding B), and the realm
/// grants both routes. The cold recovery keeps A: the turn is not served on
/// B, which no recovery overrides were asked to select.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_governed_cold_recovery_keeps_the_persisted_selection_over_another_host_default() {
    stock_governed_cold_turn(false).await;
}

/// A cold governed turn on the second host. With `same_binding`, both hosts
/// resolve the realm's one configured binding; without it, the second host's
/// default controller names a different binding than the session's own.
#[cfg(feature = "mcp")]
async fn stock_governed_cold_turn(same_binding: bool) {
    use meerkat::governed_realm::stock::issuance::{
        DEFAULT_STOCK_AGENT, McpOperations, StockGrantRequest, StockGrantTarget, grant_stock_agent,
    };
    use meerkat::governed_realm::stock::{StockGovernance, StockPrincipals, govern_stock_realm};
    use meerkat::governed_realm::{RealmGovernance, open_governed_realm_persistence_with_provider};
    const REALM: &str = "stock-cold";
    let mut http = Server::start().await;
    let root = tempfile::tempdir().unwrap();
    let markers = tempfile::tempdir().unwrap();
    let installation = |name: &str| {
        let mut server = sentinel_server(&markers.path().join(name));
        server.name = name.into();
        server.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
        server
    };
    let selected_server = installation("selected");
    let unselected_server = installation("unselected");
    let paths = meerkat_store::realm_paths_in(root.path(), REALM);
    let open = |governance: RealmGovernance| {
        open_governed_realm_persistence_with_provider(
            &meerkat::DiskStorageProvider,
            root.path(),
            REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
            governance,
        )
    };
    drop(open(RealmGovernance::Ungoverned).await.unwrap());
    let realm_config = meerkat_core::ConfigRuntime::new(
        Arc::new(meerkat_core::FileConfigStore::new(
            paths.config_path.clone(),
            meerkat_models::canonical(),
        )),
        paths.config_path.with_file_name("config_state.json"),
    );
    for server in [&selected_server, &unselected_server] {
        meerkat_core::McpRealmPersistTarget::new(&realm_config)
            .add(server.clone(), None)
            .await
            .expect("realm installation");
    }
    let realm = RealmId::parse(REALM).unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    govern_stock_realm(root.path(), REALM, &principals)
        .await
        .expect("governed");
    // Each host process has its own controller client; the operator grants
    // the agent both hosts' actual routes.
    let binding = configured_binding_in(REALM);
    let second_binding = if same_binding {
        binding.clone()
    } else {
        configured_binding_in(REALM)
    };
    let clients = [
        http_client_with_binding(&http, binding),
        http_client_with_binding(&http, second_binding),
    ];
    let routes = clients.iter().map(|client| {
        use meerkat_core::AgentLlmClient as _;
        let selected = client.controller_model_selection().unwrap();
        let pin = Arc::new(meerkat_llm_core::LlmClientAdapter::new(
            Arc::clone(client),
            selected.model().to_string(),
        ))
        .pin_controller()
        .expect("the controller pin");
        meerkat::governed_realm::stock::controller::controller_route_key(&pin)
            .expect("the controller's actual route")
    });
    for target in [
        StockGrantTarget::Controller {
            routes: routes.collect(),
        },
        StockGrantTarget::Mcp {
            server: selected_server.name.clone(),
            operations: McpOperations::Exact(["read".to_string()].into()),
        },
    ] {
        grant_stock_agent(
            root.path(),
            REALM,
            &principals,
            &StockGrantRequest {
                agent: DEFAULT_STOCK_AGENT.into(),
                target,
                expires_in_ms: None,
            },
        )
        .await
        .expect("issued and selected");
    }
    http.receiver.direct_answer.store(true, Ordering::SeqCst);

    // One stock host on the realm root, with no installation configured for
    // startup.
    let start_host = |client: Arc<dyn LlmClient>| {
        let (realm, principals, http, open) = (&realm, &principals, &http, &open);
        async move {
            let governance = StockGovernance::new(realm.clone(), principals.clone());
            let (_, bundle) = open(RealmGovernance::Stock(Arc::clone(&governance)))
                .await
                .unwrap();
            let selected = client.controller_model_selection().unwrap();
            let (config, _) = governed_client_config(http, &selected, &bundle).await;
            let setup =
                stock::stock_governed_jsonl_setup(&governance, config, bundle, client, Vec::new())
                    .expect("the stock host's governed JSONL setup");
            let (client_io, server_io) = tokio::io::duplex(1 << 20);
            let (r, w) = tokio::io::split(server_io);
            let (mut server, runtime) = construct(BufReader::new(r), w, setup).unwrap();
            let server_task = tokio::spawn(async move { server.run().await });
            let (r, w) = tokio::io::split(client_io);
            (BufReader::new(r), w, server_task, runtime)
        }
    };
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();

    let [first_client, second_client] = clients;
    // First host: create the session and run one turn, then go away.
    let (mut r, mut w, mut server_task, runtime) = start_host(first_client).await;
    let first = async {
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":370,"method":"session/create","params":{
                "prompt":"cold-seed", "initial_turn":"deferred"
            }}),
        )
        .await;
        let created = response(&mut r, &mut w, 370, &effects).await;
        assert!(created.get("error").is_none(), "{created}");
        let sid = SessionId::parse(created["result"]["session_id"].as_str().unwrap()).unwrap();
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":371,"method":"turn/start","params":{
                "session_id":sid.to_string(), "prompt":"before-restart"
            }}),
        )
        .await;
        let turn = response(&mut r, &mut w, 371, &effects).await;
        assert!(turn.get("error").is_none(), "{turn}");
        sid
    };
    let first_outcome =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(60), first))
            .catch_unwind()
            .await;

    // The host goes away, whatever its scenario's outcome: connection,
    // runtime and persistence. Its server task must complete successfully
    // once the connection closes. Borrow the JoinHandle so a timeout never
    // detaches it: on failure it is aborted and joined, and the runtime is
    // still shut down, before the test fails.
    drop(w);
    drop(r);
    let completion = match tokio::time::timeout(Duration::from_secs(30), &mut server_task).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(other) => Err(format!(
            "the first host's JSONL server completion: {other:?}"
        )),
        Err(_) => {
            server_task.abort();
            let joined = tokio::time::timeout(Duration::from_secs(2), &mut server_task).await;
            Err(format!(
                "the first host's JSONL server did not stop within its bound; abort/join: {joined:?}"
            ))
        }
    };
    let shutdown = runtime.try_shutdown().await;
    drop(runtime);
    let sid = match first_outcome {
        Ok(Ok(sid)) => sid,
        Ok(Err(_)) => panic!(
            "the first host's scenario timed out; server completion: {completion:?}; \
             runtime shutdown: {shutdown:?}"
        ),
        Err(panic) => {
            eprintln!(
                "the first host's scenario failed; server completion: {completion:?}; \
                 runtime shutdown: {shutdown:?}"
            );
            std::panic::resume_unwind(panic)
        }
    };
    if let Err(failure) = completion {
        panic!("{failure}; runtime shutdown: {shutdown:?}");
    }
    shutdown.expect("the first host shuts down");

    // Second host: a plain turn/start on the cold session.
    let (mut r, mut w, server_task, runtime) = start_host(second_client).await;
    let second = async {
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":380,"method":"turn/start","params":{
                "session_id":sid.to_string(), "prompt":"after-restart"
            }}),
        )
        .await;
        let turn = response(&mut r, &mut w, 380, &effects).await;
        if !same_binding {
            // The persisted selection is kept: no different host default
            // serves the session silently, although its route is granted.
            assert!(
                turn.get("error").is_some(),
                "another host default never serves the session: {turn}"
            );
            return;
        }
        assert!(
            turn.get("error").is_none(),
            "the cold session resumes: {turn}"
        );
        assert_eq!(turn["result"]["text"], FINISHED);

        // The recovered registration carries the agent's mandate: the
        // selected installation attaches live, the unselected one is refused.
        send(&mut w, mcp_add(381, &sid, &selected_server)).await;
        let staged = response(&mut r, &mut w, 381, &effects).await;
        assert!(staged.get("error").is_none(), "{staged}");
        assert_eq!(runtime.mcp_apply_staged_for_test(&sid).await.unwrap(), 1);
        wait_for_marker(&markers.path().join("selected")).await;
        settle_mcp(&runtime, &sid).await;
        send(&mut w, mcp_add(382, &sid, &unselected_server)).await;
        let refused = response(&mut r, &mut w, 382, &effects).await;
        assert_attachment_refused(
            &refused,
            meerkat_core::mcp_attachment::McpAttachmentRefusal::Unavailable,
        );
        settle_mcp(&runtime, &sid).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!markers.path().join("unselected").exists());
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(90), second))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
}

/// A due prompt schedule on `model` whose target session is materialized on
/// demand: its fire builds a new session for the occurrence.
fn due_materialized_prompt_schedule(model: &str) -> meerkat::CreateScheduleRequest {
    meerkat::CreateScheduleRequest {
        name: Some("governed-fire".into()),
        description: None,
        trigger: meerkat::TriggerSpec::Once {
            due_at_utc: chrono::Utc::now() - chrono::Duration::seconds(1),
        },
        target: meerkat::TargetBinding::session(
            meerkat::SessionTargetBinding::materialize_on_demand(
                meerkat::SessionMaterializationSpec {
                    model: model.into(),
                    system_prompt: None,
                    max_tokens: None,
                    provider: None,
                    output_schema: None,
                    structured_output_retries: None,
                    provider_params: None,
                    comms_name: None,
                    peer_meta: None,
                    labels: Default::default(),
                    preload_skills: Vec::new(),
                    additional_instructions: Vec::new(),
                    realm_id: None,
                    instance_id: None,
                    backend: None,
                    config_generation: None,
                    keep_alive: false,
                    app_context: None,
                },
                meerkat::ScheduledSessionAction::Prompt {
                    prompt: meerkat_core::ContentInput::Text("scheduled-check".into()),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            ),
        ),
        // A loaded host may claim the fire late; it still fires.
        misfire_policy: meerkat::MisfirePolicy::CatchUpWithin {
            window_seconds: 3600,
        },
        overlap_policy: meerkat::OverlapPolicy::AllowConcurrent,
        missing_target_policy: meerkat::MissingTargetPolicy::Skip,
        labels: Default::default(),
        planning_horizon_days: Some(1),
        planning_horizon_occurrences: Some(1),
    }
}

/// The first occurrence of `schedule_id` once it has left the in-flight
/// phases.
async fn terminal_occurrence(
    schedules: &meerkat::ScheduleService,
    schedule_id: &meerkat::ScheduleId,
) -> meerkat::Occurrence {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
    loop {
        let occurrence = schedules
            .list_occurrences(schedule_id)
            .await
            .expect("occurrences")
            .into_iter()
            .next()
            .expect("the planned occurrence");
        if !matches!(
            occurrence.phase,
            meerkat::OccurrencePhase::Pending
                | meerkat::OccurrencePhase::Claimed
                | meerkat::OccurrencePhase::Dispatching
                | meerkat::OccurrencePhase::AwaitingCompletion
        ) {
            return occurrence;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the fire never terminalized: {occurrence:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Whether `runtime` holds, live or persisted, the session `occurrence`'s
/// fire materializes.
async fn fired_session_exists(
    runtime: &SessionRuntime,
    sessions: &dyn meerkat::SessionStore,
    occurrence: &meerkat::Occurrence,
) -> bool {
    let session_id = occurrence.materialized_session_id();
    runtime.session_state(&session_id).await.unwrap().is_some()
        || meerkat::SessionStore::load(sessions, &session_id)
            .await
            .unwrap()
            .is_some()
}

/// A due schedule fires on the stock host. The host binds no agent onto a
/// session it materializes for a fire, and it composes its realm's own
/// schedule service ([`meerkat::PersistenceBundle::schedule_service`]): the
/// service records the realm's agent on the schedule it creates, and the
/// host refuses the fire typed, as `schedule_mandate_unavailable`, before it
/// materializes a session or reaches the model.
#[cfg(feature = "mcp")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stock_governed_host_refuses_a_scheduled_fire_before_materializing_its_session() {
    use meerkat::governed_realm::stock::issuance::{
        DEFAULT_STOCK_AGENT, StockGrantRequest, StockGrantTarget, grant_stock_agent,
    };
    use meerkat::governed_realm::stock::{StockGovernance, StockPrincipals, govern_stock_realm};
    use meerkat::governed_realm::{RealmGovernance, open_governed_realm_persistence_with_provider};
    const REALM: &str = "stock-schedule";
    let mut http = Server::start().await;
    let root = tempfile::tempdir().unwrap();
    let open = |governance: RealmGovernance| {
        open_governed_realm_persistence_with_provider(
            &meerkat::DiskStorageProvider,
            root.path(),
            REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
            governance,
        )
    };
    drop(open(RealmGovernance::Ungoverned).await.unwrap());
    let realm = RealmId::parse(REALM).unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    govern_stock_realm(root.path(), REALM, &principals)
        .await
        .expect("governed");
    // The operator grants the agent its controller's actual route, so an
    // interactive prompt would be served.
    let client = http_client_in(&http, REALM);
    let selected = client.controller_model_selection().unwrap();
    let pin = {
        use meerkat_core::AgentLlmClient as _;
        Arc::new(meerkat_llm_core::LlmClientAdapter::new(
            Arc::clone(&client),
            selected.model().to_string(),
        ))
        .pin_controller()
        .expect("the controller pin")
    };
    let route = meerkat::governed_realm::stock::controller::controller_route_key(&pin)
        .expect("the controller's actual route");
    grant_stock_agent(
        root.path(),
        REALM,
        &principals,
        &StockGrantRequest {
            agent: DEFAULT_STOCK_AGENT.into(),
            target: StockGrantTarget::Controller {
                routes: [route].into(),
            },
            expires_in_ms: None,
        },
    )
    .await
    .expect("issued and selected");

    let governance = StockGovernance::new(realm.clone(), principals.clone());
    let (_, bundle) = open(RealmGovernance::Stock(Arc::clone(&governance)))
        .await
        .unwrap();
    let sessions = bundle.session_store();
    let (config, _) = governed_client_config(&http, &selected, &bundle).await;
    let setup = stock::stock_governed_jsonl_setup(&governance, config, bundle, client, Vec::new())
        .expect("the stock host's governed JSONL setup");
    http.receiver.direct_answer.store(true, Ordering::SeqCst);
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (r, w) = tokio::io::split(server_io);
    let (mut server, runtime) = construct(BufReader::new(r), w, setup).unwrap();
    let server_task = tokio::spawn(async move { server.run().await });
    let (r, mut w) = tokio::io::split(client_io);
    let mut r = BufReader::new(r);
    let effects: Arc<Mutex<Vec<String>>> = Arc::default();
    let scenario = async {
        // The host's first session arms its schedule host.
        send(
            &mut w,
            json!({"jsonrpc":"2.0","id":390,"method":"session/create","params":{
                "prompt":"deferred-seed-schedule", "initial_turn":"deferred"
            }}),
        )
        .await;
        let created = response(&mut r, &mut w, 390, &effects).await;
        assert!(created.get("error").is_none(), "{created}");

        let schedules = runtime.schedule_service();
        let schedule = schedules
            .create(due_materialized_prompt_schedule(selected.model()))
            .await
            .expect("the schedule is created");
        let occurrence = terminal_occurrence(&schedules, &schedule.schedule_id).await;
        let observed = (
            schedule.config.commissioning_agent.clone(),
            occurrence.phase,
            serde_json::to_value(occurrence.failure_class).unwrap(),
            fired_session_exists(&runtime, sessions.as_ref(), &occurrence).await,
            http.receiver.bodies.lock().unwrap().len(),
        );
        eprintln!(
            "governed RPC fire: commissioning_agent={:?} phase={:?} failure_class={} \
             session_materialized={} model_requests={} detail={:?}",
            observed.0, observed.1, observed.2, observed.3, observed.4, occurrence.failure_detail
        );
        assert_eq!(
            observed,
            (
                Some(principals.executor.clone()),
                meerkat::OccurrencePhase::DeliveryFailed,
                json!("schedule_mandate_unavailable"),
                false,
                0,
            ),
            "detail: {:?}",
            occurrence.failure_detail
        );
    };
    let result =
        std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(60), scenario))
            .catch_unwind()
            .await;
    let cleanup_failures = cleanup_jsonl(r, &mut w, server_task, &mut http).await;
    finish_scenario(result, cleanup_failures);
}

/// The control: an ordinary realm's RPC host fires the same due schedule.
/// Its schedule service records no commissioning agent and no mandate gate
/// refuses the fire: the host materializes the occurrence's session. What
/// the delivery to that session then does is not this control's concern.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_rpc_host_materializes_a_scheduled_fire_past_the_mandate_gate() {
    let root = tempfile::tempdir().unwrap();
    let (_, bundle) = meerkat::open_realm_persistence_in(
        root.path(),
        "ordinary-schedule",
        Some(meerkat_store::RealmBackend::Sqlite),
        None,
    )
    .await
    .unwrap();
    let sessions = bundle.session_store();
    let runtime = Arc::new(
        SessionRuntime::new(
            AgentFactory::minimal(),
            Config::default(),
            4,
            bundle,
            NotificationSink::noop(),
        )
        .expect("compose the RPC session runtime"),
    );
    runtime.set_default_llm_client(Some(Arc::new(meerkat_client::TestClient::for_provider(
        meerkat_core::Provider::Anthropic,
    ))));
    let scenario = async {
        runtime
            .ensure_schedule_host_started()
            .await
            .expect("the schedule host starts");
        let schedules = runtime.schedule_service();
        let schedule = schedules
            .create(due_materialized_prompt_schedule(E1_MODEL))
            .await
            .expect("the schedule is created");
        let occurrence = terminal_occurrence(&schedules, &schedule.schedule_id).await;
        eprintln!(
            "ordinary RPC fire: phase={:?} failure_class={:?} detail={:?}",
            occurrence.phase, occurrence.failure_class, occurrence.failure_detail
        );
        assert_eq!(
            (
                schedule.config.commissioning_agent.clone(),
                fired_session_exists(&runtime, sessions.as_ref(), &occurrence).await,
                occurrence.failure_class
                    == Some(meerkat::OccurrenceFailureClass::ScheduleMandateUnavailable),
            ),
            (None, true, false),
            "{occurrence:?}"
        );
    };
    let result = tokio::time::timeout(Duration::from_secs(60), scenario).await;
    let shutdown = runtime.try_shutdown().await;
    result.expect("the scenario finishes within its bound");
    shutdown.expect("the runtime shuts down");
}

/// Both RPC runtime constructors compose the realm's own schedule service. A
/// stock-governed realm's records the realm's agent on each schedule it
/// creates or redefines, so a redefinition through the RPC host keeps the
/// agent another host of the realm recorded rather than erasing it; an
/// ordinary realm's records none.
#[tokio::test]
async fn rpc_runtimes_compose_the_realms_own_schedule_service() {
    use meerkat::governed_realm::stock::{StockGovernance, StockPrincipals, govern_stock_realm};
    use meerkat::governed_realm::{RealmGovernance, open_governed_realm_persistence_with_provider};
    const REALM: &str = "stock-schedule-service";
    let root = tempfile::tempdir().unwrap();
    let realm = RealmId::parse(REALM).unwrap();
    let principals = StockPrincipals::for_user(&realm, 4242).unwrap();
    let open = |governance: RealmGovernance| {
        open_governed_realm_persistence_with_provider(
            &meerkat::DiskStorageProvider,
            root.path(),
            REALM,
            Some(meerkat_store::RealmBackend::Sqlite),
            None,
            None,
            governance,
        )
    };
    let compose = |with_config_store: bool, bundle: PersistenceBundle| {
        if with_config_store {
            SessionRuntime::new_with_config_store(
                AgentFactory::minimal(),
                Config::default(),
                Arc::new(MemoryConfigStore::new(
                    Config::default(),
                    meerkat_models::canonical(),
                )),
                1,
                bundle,
                NotificationSink::noop(),
            )
        } else {
            SessionRuntime::new(
                AgentFactory::minimal(),
                Config::default(),
                1,
                bundle,
                NotificationSink::noop(),
            )
        }
        .expect("compose the RPC session runtime")
    };
    for governed in [false, true] {
        if governed {
            govern_stock_realm(root.path(), REALM, &principals)
                .await
                .expect("governed");
        }
        let recorded = governed.then(|| principals.executor.clone());
        for with_config_store in [false, true] {
            let governance = if governed {
                RealmGovernance::Stock(StockGovernance::new(realm.clone(), principals.clone()))
            } else {
                RealmGovernance::Ungoverned
            };
            let (_, bundle) = open(governance).await.unwrap();
            let runtime = compose(with_config_store, bundle);
            let schedules = runtime.schedule_service();
            // Another host of the realm records what its own service
            // commissions: the realm's agent in a stock-governed realm.
            let other_host = match recorded.clone() {
                Some(agent) => meerkat::ScheduleService::new_commissioned(schedules.store(), agent),
                None => meerkat::ScheduleService::new(schedules.store()),
            };
            let created = other_host
                .create(due_materialized_prompt_schedule(E1_MODEL))
                .await
                .expect("the schedule is created");
            let redefined = schedules
                .update(
                    &created.schedule_id,
                    meerkat::UpdateScheduleRequest {
                        name: Some("redefined".into()),
                        ..meerkat::UpdateScheduleRequest::default()
                    },
                )
                .await
                .expect("the schedule is redefined");
            assert_eq!(
                (
                    schedules.commissioning_agent().cloned(),
                    redefined.config.commissioning_agent,
                ),
                (recorded.clone(), recorded.clone()),
                "governed={governed} with_config_store={with_config_store}"
            );
            runtime.try_shutdown().await.unwrap();
        }
    }
}
