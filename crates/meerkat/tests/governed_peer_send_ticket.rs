//! A reviewed-entry ticket that a governed peer send's dispatch minted and
//! never used cannot be entered once the run that admitted the send is
//! consumed: its first entry refuses at the run fence.
//!
//! The sender is a real factory-built agent on a governed machine (the
//! generated grant owner, native admission and the run's custody), with its
//! actual comms tool surface. The test attaches its dispatch admission
//! through the agent's build options, so the factory installs it as the
//! outermost call gate, the same seam a host uses. The admission keeps the
//! exact dispatch's ticket without entering it. The destination is a real
//! session on an ungoverned machine that receives the delivery through its
//! own peer ingress.
#![cfg(all(
    not(target_arch = "wasm32"),
    feature = "local-authorization",
    feature = "comms"
))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use meerkat::{AgentFactory, EphemeralSessionService, FactoryAgentBuilder};
use meerkat_authorization::clock::HostAuthorizationClock;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance, single_tool_operation_target,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
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
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse,
    OperationObservedOutcome, OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
    ToolAuthorizationTarget,
};
use meerkat_core::comms::{PeerAddress, PeerName, PeerTransport, TrustedPeerDescriptor};
use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
};
use meerkat_core::{
    AgentError, AgentSessionStore, Config, ControllerModelSelection, LiveBridgeEffectKind,
    LiveBridgeEffectOutcome, Message, PrincipalKind, PrincipalRef, Provider, ReviewedEntryTicket,
    RunId, Session, SessionId, SessionLlmIdentity, StopReason, ToolCallView, ToolDispatchAdmission,
    ToolDispatchContext, ToolError, ToolOperationTarget, ToolResult, TrustDomainId,
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
use tokio::sync::Notify;

const MODEL: &str = "claude-sonnet-4-5";
const ENDPOINT: &str = "https://governed-peer-send.invalid/messages";
const SEND_CALL: &str = "send-to-destination";
const BODY: &str = "governed peer send body";
const BOUND: Duration = Duration::from_secs(30);

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded fixture identifier")
}

fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("governed-peer-send").expect("domain"),
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
        digest: EvidenceDigest::from_array([3; 32]),
    }
}

fn identity() -> SessionLlmIdentity {
    SessionLlmIdentity {
        model: MODEL.into(),
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
}

fn credential() -> AuthCredentialIdentity {
    serde_json::from_value(
        serde_json::json!({"realm": "governed-peer-send", "account": "scripted-controller"}),
    )
    .expect("nonsecret identity of the fixture transport")
}

fn selection() -> ControllerModelSelection {
    ControllerModelSelection::new(
        identity(),
        credential(),
        "scripted-profile".into(),
        "scripted-transport".into(),
    )
}

fn model_facts() -> ModelAuthorizationFacts {
    ModelAuthorizationFacts {
        identity: Arc::new(identity()),
        wire_model: MODEL.into(),
        hosted_capabilities: Arc::from([]),
        backend_profile_id: Some("scripted-profile".into()),
        backend_kind: "scripted-transport".into(),
        endpoint: ENDPOINT.into(),
        credential: Some(credential()),
        usage: ModelAuthorizationUse::ControllerInference,
        live_channel: None,
    }
}

fn action(value: &str) -> ActionRef {
    ActionRef {
        feature: "governed-peer-send".into(),
        action: value.into(),
    }
}

fn domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("resource-owner"),
        namespace: "peers".into(),
    }
}

fn operation_values(verb: &str) -> LocalOperationValues {
    LocalOperationValues {
        action: action(verb),
        resource_domain: domain(),
        processor: ProcessorRef::Principal {
            principal: principal("executor"),
        },
        audience: AudienceRef::Principal {
            principal: principal("requester"),
        },
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

/// The application entitlement of the admitted work: the fixture's one
/// requester, ingress and executor.
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

    fn review_mandate(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        Ok(None)
    }
}

/// Maps the controller's inference and exactly one operation: a message
/// delivery to the destination peer. Everything else is denied.
struct PeerSendOwner {
    destination: Arc<OnceLock<meerkat_core::comms::PeerId>>,
}

impl OperationPolicyOwner for PeerSendOwner {
    fn authorize_controller_admission(
        &self,
        _: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        _: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        if facts.selection() != &selection()
            || facts.endpoint() != ENDPOINT
            || facts.wire_model() != MODEL
        {
            return Err(denied().into());
        }
        Ok(
            meerkat_authorization::grant_policy::ControllerAdmissionAllowance {
                operation_values: vec![operation_values("infer")],
                restrictions: ExecutionRestrictions::unrestricted(),
            },
        )
    }

    fn authorize_operation(
        &self,
        _: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let verb = match &binding.facts().operation {
            AuthorizationOperation::Model(facts)
                if purpose == LocalPolicyPurpose::Controller
                    && selection().matches_model_facts(facts)
                    && facts.endpoint.as_ref() == ENDPOINT
                    && facts.wire_model.as_ref() == MODEL
                    && facts.hosted_capabilities.is_empty()
                    && facts.live_channel.is_none() =>
            {
                "infer"
            }
            AuthorizationOperation::Tool(facts)
                if purpose == LocalPolicyPurpose::Operation && facts.name == "send_message" =>
            {
                let ToolAuthorizationTarget::Dispatcher(plan) = &facts.target else {
                    return Err(denied().into());
                };
                match single_tool_operation_target(plan)? {
                    Some(ToolOperationTarget::PeerSend { peer, kind })
                        if Some(peer) == self.destination.get()
                            && *kind == meerkat_core::PeerSendKind::Message =>
                    {
                        "send"
                    }
                    _ => return Err(denied().into()),
                }
            }
            _ => return Err(denied().into()),
        };
        Ok(LocalPolicyAllowance {
            operation_values: vec![operation_values(verb)],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

/// The sessions' store, in memory: nothing is written under the test's
/// working directory.
#[derive(Default)]
struct MemorySessions(Mutex<HashMap<SessionId, Session>>);

#[async_trait]
impl AgentSessionStore for MemorySessions {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.0
            .lock()
            .expect("fixture session store")
            .insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        Ok(SessionId::parse(id).ok().and_then(|id| {
            self.0
                .lock()
                .expect("fixture session store")
                .get(&id)
                .cloned()
        }))
    }
}

fn done(stop_reason: StopReason) -> Result<LlmEvent, LlmError> {
    Ok(LlmEvent::Done {
        outcome: LlmDoneOutcome::success(stop_reason),
    })
}

fn text(reply: &str) -> Vec<Result<LlmEvent, LlmError>> {
    vec![
        Ok(LlmEvent::TextDelta {
            delta: reply.into(),
            meta: None,
        }),
        done(StopReason::EndTurn),
    ]
}

fn tool_feedback(messages: &[Message], call_id: &str) -> Option<ToolResult> {
    messages.iter().find_map(|message| match message {
        Message::ToolResults { results, .. } => results
            .iter()
            .find(|result| result.tool_use_id == call_id)
            .cloned(),
        _ => None,
    })
}

/// The sender's controller transport: its governed first request calls
/// `send_message` to the destination, and its second ends the turn once the
/// delivery's result came back as an ordinary success.
struct SenderTransport {
    destination: Arc<OnceLock<meerkat_core::comms::PeerId>>,
    requests: AtomicUsize,
}

#[async_trait]
impl LlmClient for SenderTransport {
    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        meerkat_llm_core::PlainModelRoute::new(ENDPOINT, MODEL)
    }

    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(selection())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        panic!("the governed sender's request must reach the prepared provider entry")
    }

    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let check = request
            .authorization()
            .expect("native work reaches the provider")
            .prepare(model_facts())
            .expect("native controller authorization")
            .current()
            .expect("final controller entry check");
        check
            .observe_entry(meerkat_core::OperationEntryBoundary::ModelRequest)
            .expect("native audit stages the transport entry");
        check
            .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
            .expect("native audit stages the response");
        match self.requests.fetch_add(1, Ordering::SeqCst) {
            0 => {
                let peer = self
                    .destination
                    .get()
                    .expect("destination peer known before admission");
                Box::pin(futures::stream::iter(vec![
                    Ok(LlmEvent::ToolCallComplete {
                        id: SEND_CALL.into(),
                        name: "send_message".into(),
                        args: serde_json::json!({
                            "peer_id": peer.to_string(),
                            "body": BODY,
                            "handling_mode": "queue",
                        }),
                        meta: None,
                    }),
                    done(StopReason::ToolUse),
                ]))
            }
            1 => {
                let feedback = tool_feedback(&request.request().messages, SEND_CALL)
                    .expect("the delivery's result returned to the same loop");
                assert!(
                    !feedback.is_error,
                    "the governed delivery succeeded: {}",
                    feedback.text_content()
                );
                Box::pin(futures::stream::iter(text("delivered")))
            }
            _ => panic!("unexpected extra sender request"),
        }
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// The ungoverned destination's transport: it acknowledges what it is
/// delivered.
struct DestinationTransport;

#[async_trait]
impl LlmClient for DestinationTransport {
    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        meerkat_llm_core::PlainModelRoute::new(ENDPOINT, MODEL)
    }

    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(selection())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        Box::pin(futures::stream::iter(text("acknowledged")))
    }

    fn stream_prepared<'a>(&'a self, _: &'a PreparedLlmRequest) -> LlmStream<'a> {
        Box::pin(futures::stream::iter(text("acknowledged")))
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// The dispatch admission the test attaches through the agent's build
/// options. On the governed delivery it checks the exact call and keeps the
/// exact dispatch's single-use ticket without entering it; after the actual
/// send returned, it checks the call is still current and holds the
/// dispatch until the test releases it.
struct PeerSendTicketCapture {
    ticket: Mutex<Option<ReviewedEntryTicket>>,
    run: Mutex<Option<RunId>>,
    admissions: AtomicUsize,
    outcomes: AtomicUsize,
    after_effect: Notify,
    held: AtomicBool,
    release: tokio::sync::watch::Sender<bool>,
}

impl PeerSendTicketCapture {
    fn new() -> Self {
        Self {
            ticket: Mutex::new(None),
            run: Mutex::new(None),
            admissions: AtomicUsize::new(0),
            outcomes: AtomicUsize::new(0),
            after_effect: Notify::new(),
            held: AtomicBool::new(false),
            release: tokio::sync::watch::channel(false).0,
        }
    }
}

#[async_trait]
impl ToolDispatchAdmission for PeerSendTicketCapture {
    async fn await_dispatch_admission(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        _: LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        assert_eq!(call.name, "send_message");
        assert_eq!(self.admissions.fetch_add(1, Ordering::SeqCst), 0);
        let context = context.expect("the governed delivery's dispatch context");
        assert!(context.work_authorization().is_some());
        context.check_tool_authorization(call, None)?;
        let ticket = context.reviewed_entry_ticket(call, None)?;
        assert!(self.ticket.lock().unwrap().replace(ticket).is_none());
        *self.run.lock().unwrap() = Some(context.run_id().expect("the native run").clone());
        Ok(())
    }

    async fn record_dispatch_outcome(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        _: LiveBridgeEffectKind,
        outcome: LiveBridgeEffectOutcome,
    ) -> Result<(), ToolError> {
        assert_eq!(call.name, "send_message");
        assert_eq!(outcome, LiveBridgeEffectOutcome::Committed);
        // The actual send returned. The exact prepared check is still
        // current, and the kept ticket has never been entered.
        context
            .expect("the governed delivery's dispatch context")
            .check_tool_authorization(call, None)?;
        assert_eq!(self.outcomes.fetch_add(1, Ordering::SeqCst), 0);
        self.held.store(true, Ordering::SeqCst);
        self.after_effect.notify_one();
        let mut release = self.release.subscribe();
        while !*release.borrow_and_update() {
            release.changed().await.expect("release sender kept");
        }
        Ok(())
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
            work: id("peer-send-work"),
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
            realm: RealmId::parse("governed-peer-send").expect("realm"),
            ingress: ResourceDomain {
                authority: principal("ingress"),
                namespace: "prompts".into(),
            },
            occurrence_scope: id("occurrence-1"),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .expect("immutable association claims")
}

fn deferred_session(
    comms_name: &str,
    admission: Option<Arc<dyn ToolDispatchAdmission>>,
) -> CreateSessionRequest {
    CreateSessionRequest {
        model: MODEL.into(),
        prompt: "".into(),
        injected_context: Vec::new(),
        system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: InitialTurnPolicy::Defer,
        deferred_prompt_policy: DeferredPromptPolicy::Discard,
        build: Some(SessionBuildOptions {
            comms_name: Some(comms_name.into()),
            tool_dispatch_admission: admission,
            ..Default::default()
        }),
        labels: None,
    }
}

/// The trusted-peer descriptor of a session's actual in-process comms
/// runtime.
fn descriptor(runtime: &Arc<dyn meerkat_core::agent::CommsRuntime>) -> TrustedPeerDescriptor {
    let name = runtime.comms_name().expect("named comms runtime");
    TrustedPeerDescriptor {
        peer_id: runtime.peer_id().expect("comms identity"),
        name: PeerName::new(name.clone()).expect("peer name"),
        address: PeerAddress::new(PeerTransport::Inproc, name),
        pubkey: runtime.public_key_bytes().expect("signing key"),
    }
}

async fn input_census(machine: &MeerkatMachine, session: &SessionId) -> Vec<String> {
    let mut rows: Vec<String> = machine
        .session_input_states(session)
        .await
        .expect("input census")
        .report
        .into_iter()
        .map(|row| {
            format!(
                "{} {:?} {:?}",
                row.state.input_id, row.seed.phase, row.seed.terminal_outcome
            )
        })
        .collect();
    rows.sort();
    rows
}

/// The session's native input ids, sorted: which inputs it holds, whatever
/// lifecycle step each has reached.
async fn input_ids(machine: &MeerkatMachine, session: &SessionId) -> Vec<String> {
    let mut ids: Vec<String> = machine
        .session_input_states(session)
        .await
        .expect("input census")
        .report
        .into_iter()
        .map(|row| row.state.input_id.to_string())
        .collect();
    ids.sort();
    ids
}

/// A governed send's ticket the dispatch kept unused: after its run is
/// consumed, its first entry refuses as malformed facts at the run fence,
/// not as an ended dispatch. The delivery itself reached the destination
/// exactly once, and the refused entry adds no input or effect anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unused_peer_send_ticket_refuses_after_its_run_is_consumed() {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("peer-send-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("generated grant owner"),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .expect("controller lineage");
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("peer-send"),
            principal("executor"),
            None,
            ceiling("send"),
        )
        .expect("peer-send operation grant");
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        let candidate = claimed.candidate();
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || candidate.logical_executor != principal("executor")
            || candidate.target.logical_runtime != id(&runtime.to_string())
            || candidate.controller_model.as_ref() != Some(&selection())
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let destination_peer = Arc::new(OnceLock::new());
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                retained_resume: None,
                require_scoped_destination: false,
                grants,
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(PeerSendOwner {
                    destination: Arc::clone(&destination_peer),
                }),
                configured_mcp_attachment: None,
            })
            .expect("governed native host"),
    );

    // The destination: an ordinary session on an ungoverned machine whose
    // peer ingress is current.
    let destination_machine = Arc::new(MeerkatMachine::ephemeral());
    let mut destination_builder =
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
    destination_builder.default_llm_client = Some(Arc::new(DestinationTransport));
    destination_builder.default_session_store = Some(Arc::new(MemorySessions::default()));
    let destination_service = Arc::new(EphemeralSessionService::new(destination_builder, 2));
    let destination = meerkat::surface::materialize_ephemeral_runtime_session(
        &destination_service,
        &destination_machine,
        deferred_session("governed-peer-destination", None),
        true,
    )
    .await
    .expect("destination session")
    .session_id;
    let destination_comms = destination_service
        .comms_runtime(&destination)
        .await
        .expect("destination comms runtime");

    // The sender: the governed agent whose build carries the capture.
    let capture = Arc::new(PeerSendTicketCapture::new());
    let transport = Arc::new(SenderTransport {
        destination: Arc::clone(&destination_peer),
        requests: AtomicUsize::new(0),
    });
    let mut builder =
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
    builder.default_llm_client = Some(transport.clone());
    builder.default_session_store = Some(Arc::new(MemorySessions::default()));
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let sender = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        deferred_session(
            "governed-peer-sender",
            Some(Arc::clone(&capture) as Arc<dyn ToolDispatchAdmission>),
        ),
        false,
    )
    .await
    .expect("governed sender session")
    .session_id;
    let sender_comms = service
        .comms_runtime(&sender)
        .await
        .expect("sender comms runtime");

    // Each side trusts the other through its own machine's generated trust.
    let destination_descriptor = descriptor(&destination_comms);
    destination_peer
        .set(destination_descriptor.peer_id)
        .expect("destination peer recorded once");
    machine
        .stage_add_direct_peer_endpoint(
            &sender,
            meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&destination_descriptor),
            Arc::clone(&sender_comms),
        )
        .await
        .expect("the sender trusts the destination");
    destination_machine
        .stage_add_direct_peer_endpoint(
            &destination,
            meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&descriptor(&sender_comms)),
            Arc::clone(&destination_comms),
        )
        .await
        .expect("the destination trusts the sender");

    let actor = service
        .live_session_actor_witness(&sender)
        .await
        .expect("live sender actor");
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .expect("selected controller client");
    assert_eq!(pin.selection(), &selection());
    let credential_owner = machine.generated_auth_lease_handle();
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &credential_owner,
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-peer-send-credential"),
        tokio::time::Instant::now() + BOUND,
    )
    .await
    .expect("fixture credential before native admission");

    let runtime = LogicalRuntimeId::for_session(&sender);
    let mut prompt = PromptInput::new("Tell the destination", None);
    prompt.header.authority_association = Some(association(
        &runtime,
        controller,
        operation,
        pin.selection().clone(),
    ));
    let input = Input::Prompt(prompt);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("governed-peer-send").expect("realm"),
        evidence("fresh-transport-authentication"),
    )
    .expect("trusted ingress observation")
    .with_controller_client(&input, pin)
    .expect("selected client bound before admission");
    let input = input
        .with_ingress_context(current)
        .expect("exact final input");
    let (_, completion) = machine
        .accept_input_with_completion(&sender, input)
        .await
        .expect("governed admission");
    let completion = completion.expect("completion waiter");

    // The actual send returned and the dispatch is held after it.
    tokio::time::timeout(BOUND, capture.after_effect.notified())
        .await
        .expect("the governed delivery committed");
    assert!(capture.held.load(Ordering::SeqCst));
    let destination_census = tokio::time::timeout(BOUND, async {
        loop {
            let census = input_census(&destination_machine, &destination).await;
            if !census.is_empty() {
                return census;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the destination received the delivery");
    assert_eq!(
        destination_census.len(),
        1,
        "exactly one native input at the destination: {destination_census:?}"
    );

    capture.release.send_replace(true);
    let outcome = tokio::time::timeout(BOUND, completion.wait())
        .await
        .expect("the sender's run ended within bound");
    assert!(
        matches!(outcome, Ok(CompletionOutcome::Completed(_))),
        "the sender's run completed: {outcome:?}"
    );
    let sender_census = input_census(&machine, &sender).await;
    // The sender's input is terminal once its completion resolves. The
    // destination's own run may still be consuming the delivered input, so
    // only which inputs the destination holds is fixed here, not how far
    // each has run.
    let destination_inputs = input_ids(&destination_machine, &destination).await;
    assert_eq!(
        destination_inputs.len(),
        1,
        "the delivery reached the destination once: {destination_inputs:?}"
    );

    let ticket = capture
        .ticket
        .lock()
        .unwrap()
        .take()
        .expect("the kept ticket");
    let entered = ticket.enter();
    assert!(
        matches!(
            &entered,
            Err(ToolError::AuthorizationRefused { refusal, .. })
                if refusal.kind() == OperationRefusalKind::MalformedFacts
        ),
        "the first entry after the run is consumed refuses at the run fence: {entered:?}"
    );
    assert_eq!(capture.admissions.load(Ordering::SeqCst), 1);
    assert_eq!(capture.outcomes.load(Ordering::SeqCst), 1);
    assert_eq!(transport.requests.load(Ordering::SeqCst), 2);
    assert_eq!(
        input_census(&machine, &sender).await,
        sender_census,
        "the refused entry adds no sender input"
    );
    assert_eq!(
        input_ids(&destination_machine, &destination).await,
        destination_inputs,
        "the refused entry delivers nothing more"
    );
}
