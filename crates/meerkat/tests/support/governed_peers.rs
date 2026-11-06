//! One governed machine hosting two agents in one process: a sending agent
//! whose governed work asks a receiving agent through the comms tools, over
//! the in-process lane. The machine runs the actual native admission, grant
//! owner and run custody. The test installs the receiving side's peer
//! ingress binder before any session registers.
//!
//! Roles: `requester` asks the sending agent (`data-agent`) through the
//! host's `ingress`; the sending agent asks the receiving agent
//! (`owner-agent`) with `send_message`.
#![allow(dead_code, clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
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
use meerkat_core::comms::{PeerAddress, PeerId, PeerName, PeerTransport, TrustedPeerDescriptor};
use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
use meerkat_core::service::{
    CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
};
use meerkat_core::{
    AgentError, AgentSessionStore, Config, ControllerModelClient, ControllerModelSelection,
    Message, PrincipalKind, PrincipalRef, Provider, Session, SessionId, SessionLlmIdentity,
    StopReason, ToolOperationTarget, ToolResult, TrustDomainId,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PreparedLlmRequest,
};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_runtime::input_authority::{NativeIngressContext, PeerIngressBinder};
use meerkat_runtime::meerkat_machine::{
    MeerkatMachine, NativeGrantWorkConfiguration, NativeIngressCheck,
};

pub const MODEL: &str = "claude-sonnet-4-5";
pub const ENDPOINT: &str = "https://governed-peers.invalid/messages";
pub const REALM: &str = "governed-peers";
/// The requester's question to the sending agent.
pub const SENDER_PROMPT: &str = "Ask the owner's agent for the quarterly record";
/// What the sending agent sends the receiving agent.
pub const BODY: &str = "Please send the quarterly record for my requester";
/// The sending agent's call id for its one delivery.
pub const SEND_CALL: &str = "ask-the-owner-agent";
pub const BOUND: Duration = Duration::from_secs(30);

pub fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded fixture identifier")
}

pub fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("governed-peers").expect("domain"),
    )
    .expect("qualified fixture principal")
}

/// The person who asks the sending agent.
pub fn requester() -> PrincipalRef {
    principal("requester")
}

/// The host's ingress that authenticated the requester.
pub fn ingress() -> PrincipalRef {
    principal("ingress")
}

/// The sending agent.
pub fn sender_agent() -> PrincipalRef {
    principal("data-agent")
}

/// The receiving agent.
pub fn receiver_agent() -> PrincipalRef {
    principal("owner-agent")
}

pub fn realm() -> RealmId {
    RealmId::parse(REALM).expect("realm")
}

/// The host's one installed ingress domain.
pub fn ingress_domain() -> ResourceDomain {
    ResourceDomain {
        authority: ingress(),
        namespace: "prompts".into(),
    }
}

pub fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

pub fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: ingress(),
                namespace: "observations".into(),
            },
            resource_id: value.into(),
        },
        revision: id("observation-1"),
        digest: EvidenceDigest::from_array([5; 32]),
    }
}

pub fn identity() -> SessionLlmIdentity {
    SessionLlmIdentity {
        model: MODEL.into(),
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
}

fn credential() -> AuthCredentialIdentity {
    serde_json::from_value(serde_json::json!({"realm": REALM, "account": "scripted-controller"}))
        .expect("nonsecret identity of the fixture transport")
}

pub fn selection() -> ControllerModelSelection {
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

fn action(verb: &str) -> ActionRef {
    ActionRef {
        feature: "governed-peers".into(),
        action: verb.into(),
    }
}

fn domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("resource-owner"),
        namespace: "records".into(),
    }
}

fn values(verb: &str, executor: &PrincipalRef) -> LocalOperationValues {
    LocalOperationValues {
        action: action(verb),
        resource_domain: domain(),
        processor: ProcessorRef::Principal {
            principal: executor.clone(),
        },
        audience: AudienceRef::Principal {
            principal: requester(),
        },
    }
}

/// Bounds that admit exactly `verb` by `executor` for the requester.
pub fn ceiling(verb: &str, executor: &PrincipalRef) -> ExecutionRestrictions {
    let mut bounds = ExecutionRestrictions::unrestricted();
    bounds.actions = ExactRestriction::exact([action(verb)]);
    bounds.resource_domains = DomainRestriction::exact([domain()]);
    bounds.processors = ExactRestriction::exact([ProcessorRef::Principal {
        principal: executor.clone(),
    }]);
    bounds.audiences = ExactRestriction::exact([AudienceRef::Principal {
        principal: requester(),
    }]);
    bounds
}

/// The host's application entitlement: the requester's work on the sending
/// agent, and every work admitted to the receiving agent. This fixture's
/// host decides nothing about who a peer work's requester is: that is the
/// binding's, and Core's checks of it.
struct Invocation;

impl AdmittedWorkPolicyOwner for Invocation {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        let claims = association.candidate();
        let sender_work = claims.requester == requester()
            && claims.ingress_actor == ingress()
            && claims.logical_executor == sender_agent();
        if !sender_work && claims.logical_executor != receiver_agent() {
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

/// Maps both agents' controller inference and exactly one operation: the
/// sending agent's message delivery to the receiving agent.
struct Operations {
    receiver_peer: Arc<OnceLock<PeerId>>,
}

impl OperationPolicyOwner for Operations {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
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
                operation_values: vec![values("infer", &association.candidate().logical_executor)],
                restrictions: ExecutionRestrictions::unrestricted(),
            },
        )
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let executor = &association.candidate().logical_executor;
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
                if purpose == LocalPolicyPurpose::Operation
                    && facts.name == "send_message"
                    && executor == &sender_agent() =>
            {
                let ToolAuthorizationTarget::Dispatcher(plan) = &facts.target else {
                    return Err(denied().into());
                };
                match single_tool_operation_target(plan)? {
                    Some(ToolOperationTarget::PeerSend { peer, kind })
                        if Some(peer) == self.receiver_peer.get()
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
            operation_values: vec![values(verb, executor)],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

/// The sessions' store, in memory.
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

fn text(reply: &str) -> LlmStream<'static> {
    Box::pin(futures::stream::iter(vec![
        Ok(LlmEvent::TextDelta {
            delta: reply.into(),
            meta: None,
        }),
        done(StopReason::EndTurn),
    ]))
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

fn mentions(messages: &[Message], needle: &str) -> bool {
    messages.iter().any(|message| {
        serde_json::to_string(message)
            .expect("message renders")
            .contains(needle)
    })
}

/// Both agents' controller transport. Governed requests only. The sending
/// agent's first request (it carries the requester's prompt) calls
/// `send_message` to the receiving agent; its next ends the turn once the
/// delivery's result came back. Any other request is the receiving agent's
/// turn on what it was delivered.
pub struct ScriptedAgents {
    receiver_peer: Arc<OnceLock<PeerId>>,
    /// Further `send_message` arguments the model writes beside the
    /// addressee and body: claims a model may make in a tool call.
    extra_arguments: Mutex<serde_json::Map<String, serde_json::Value>>,
    sender_feedback: Mutex<Vec<ToolResult>>,
    receiver_turns: AtomicUsize,
    requests: AtomicUsize,
}

impl ScriptedAgents {
    fn new(receiver_peer: Arc<OnceLock<PeerId>>) -> Self {
        Self {
            receiver_peer,
            extra_arguments: Mutex::new(serde_json::Map::new()),
            sender_feedback: Mutex::new(Vec::new()),
            receiver_turns: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
        }
    }

    /// The model also writes `arguments` into its `send_message` call.
    pub fn also_write(&self, arguments: serde_json::Value) {
        let serde_json::Value::Object(arguments) = arguments else {
            panic!("tool arguments are an object");
        };
        self.extra_arguments
            .lock()
            .expect("arguments")
            .extend(arguments);
    }

    /// What the sending agent's delivery returned to its loop.
    pub fn sender_feedback(&self) -> Vec<ToolResult> {
        self.sender_feedback.lock().expect("feedback").clone()
    }

    /// How many turns the receiving agent ran on what it was delivered.
    pub fn receiver_turns(&self) -> usize {
        self.receiver_turns.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LlmClient for ScriptedAgents {
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
        panic!("governed work must reach the prepared provider entry")
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
        self.requests.fetch_add(1, Ordering::SeqCst);
        let messages = &request.request().messages;
        if let Some(feedback) = tool_feedback(messages, SEND_CALL) {
            self.sender_feedback
                .lock()
                .expect("feedback")
                .push(feedback);
            return text("asked");
        }
        if mentions(messages, SENDER_PROMPT) {
            let peer = self
                .receiver_peer
                .get()
                .expect("the receiving agent's peer id is known before admission");
            let mut arguments = serde_json::Map::new();
            arguments.insert("peer_id".into(), peer.to_string().into());
            arguments.insert("body".into(), BODY.into());
            arguments.insert("handling_mode".into(), "queue".into());
            for (name, value) in self.extra_arguments.lock().expect("arguments").iter() {
                arguments
                    .entry(name.clone())
                    .or_insert_with(|| value.clone());
            }
            return Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: SEND_CALL.into(),
                    name: "send_message".into(),
                    args: serde_json::Value::Object(arguments),
                    meta: None,
                }),
                done(StopReason::ToolUse),
            ]));
        }
        self.receiver_turns.fetch_add(1, Ordering::SeqCst);
        text("answered")
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// What a peer ingress binder for the receiving agent is built over: the
/// session owner (for the receiving agent's controller pin) and the
/// receiving agent's own grants.
#[derive(Clone)]
pub struct PeerHost {
    pub service: Arc<EphemeralSessionService<FactoryAgentBuilder>>,
    pub grants: Arc<LocalGrantAuthority>,
    /// The receiving agent's controller grant.
    pub receiver_controller: GrantLineageRef,
    /// The receiving agent's own grant for serving peers.
    pub receiver_basis: GrantLineageRef,
}

impl PeerHost {
    /// The receiving session's live controller client, without waiting on
    /// its turn: a held turn boundary is `None`, as is a session that is not
    /// live.
    pub async fn try_pin(&self, session_id: &SessionId) -> Option<ControllerModelClient> {
        let actor = self.service.live_session_actor_witness(session_id).await?;
        self.service
            .try_pin_controller_client_for_actor(&actor)
            .await
            .ok()
    }

    /// The receiving session's destination, as an admission of `runtime`
    /// names it.
    pub fn target(&self, runtime: &LogicalRuntimeId) -> NativeWorkTarget {
        NativeWorkTarget {
            logical_owner: principal("native-owner"),
            logical_runtime: id(&runtime.to_string()),
            context: id("native-context"),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: requester(),
            },
        }
    }
}

/// The trusted-peer descriptor of a session's in-process comms runtime.
fn descriptor(runtime: &Arc<dyn meerkat_core::agent::CommsRuntime>) -> TrustedPeerDescriptor {
    let name = runtime.comms_name().expect("named comms runtime");
    TrustedPeerDescriptor {
        peer_id: runtime.peer_id().expect("comms identity"),
        name: PeerName::new(name.clone()).expect("peer name"),
        address: PeerAddress::new(PeerTransport::Inproc, name),
        pubkey: runtime.public_key_bytes().expect("signing key"),
    }
}

fn deferred_session(comms_name: &str) -> CreateSessionRequest {
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
            ..Default::default()
        }),
        labels: None,
    }
}

/// The two agents on their one governed machine.
pub struct Peers {
    pub machine: Arc<MeerkatMachine>,
    pub host: PeerHost,
    pub transport: Arc<ScriptedAgents>,
    pub sender: SessionId,
    pub receiver: SessionId,
    pub sender_peer: PeerId,
    pub receiver_peer: PeerId,
    /// The sending agent's controller grant.
    pub sender_controller: GrantLineageRef,
    /// The requester's delegation to the sending agent: its message to the
    /// receiving agent.
    pub sender_basis: GrantLineageRef,
}

impl Peers {
    /// Both agents, each trusting the other, on one governed machine on
    /// which `binder` (built over the machine's host) is installed before
    /// any session registers.
    pub async fn new(binder: impl FnOnce(&PeerHost) -> Arc<dyn PeerIngressBinder>) -> Self {
        let grants = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: principal("grant-owner"),
                    namespace: id("governed-peer-grants"),
                    generation: 1,
                },
                LocalAuthorizationPublication::new(),
                Arc::new(HostAuthorizationClock),
            )
            .expect("generated grant owner"),
        );
        let issue = |name: &str, grantee: PrincipalRef, bounds: ExecutionRestrictions| {
            grants
                .issue_root(&principal("grant-owner"), id(name), grantee, None, bounds)
                .expect("issued grant")
        };
        let sender_controller = issue(
            "sender-controller",
            sender_agent(),
            ceiling("infer", &sender_agent()),
        );
        let sender_basis = issue(
            "requester-delegation",
            sender_agent(),
            ceiling("send", &sender_agent()),
        );
        let receiver_controller = issue(
            "receiver-controller",
            receiver_agent(),
            ceiling("infer", &receiver_agent()),
        );
        let receiver_basis = issue(
            "owner-delegation",
            receiver_agent(),
            ceiling("infer", &receiver_agent()),
        );
        let ingress_check: Arc<NativeIngressCheck> =
            Arc::new(move |runtime, _, current, claimed| {
                let claims = claimed.candidate();
                let sender_work = current.requester() == &requester()
                    && current.ingress_actor() == &ingress()
                    && claims.logical_executor == sender_agent();
                let receiver_work = claims.logical_executor == receiver_agent();
                if !(sender_work || receiver_work)
                    || claims.target.logical_runtime != id(&runtime.to_string())
                    || claims.controller_model.as_ref() != Some(&selection())
                {
                    return Err(denied().into());
                }
                Ok(())
            });
        let receiver_peer = Arc::new(OnceLock::new());
        let machine = Arc::new(
            MeerkatMachine::ephemeral()
                .with_local_grant_authorization(NativeGrantWorkConfiguration {
                    retained_resume: None,
                    require_scoped_destination: false,
                    grants: Arc::clone(&grants),
                    ingress: ingress_check,
                    invocation_owner: Arc::new(Invocation),
                    operation_owner: Arc::new(Operations {
                        receiver_peer: Arc::clone(&receiver_peer),
                    }),
                    configured_mcp_attachment: None,
                })
                .expect("governed native host"),
        );
        let transport = Arc::new(ScriptedAgents::new(Arc::clone(&receiver_peer)));
        let mut builder =
            FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
        builder.default_llm_client = Some(transport.clone());
        builder.default_session_store = Some(Arc::new(MemorySessions::default()));
        let service = Arc::new(EphemeralSessionService::new(builder, 4));
        let host = PeerHost {
            service: Arc::clone(&service),
            grants,
            receiver_controller,
            receiver_basis,
        };
        machine
            .install_peer_ingress_binder(binder(&host))
            .expect("the binder installs before any session registers");

        // Test binaries run their tests concurrently in one process, whose
        // in-process route names are global: each fixture's names are its own.
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let materialize = |role: &'static str| {
            let service = Arc::clone(&service);
            let machine = Arc::clone(&machine);
            let comms_name = format!("governed-{role}-{suffix}");
            async move {
                meerkat::surface::materialize_ephemeral_runtime_session(
                    &service,
                    &machine,
                    deferred_session(&comms_name),
                    true,
                )
                .await
                .expect("governed session")
                .session_id
            }
        };
        let receiver = materialize("owner-agent").await;
        let sender = materialize("data-agent").await;
        let receiver_comms = service
            .comms_runtime(&receiver)
            .await
            .expect("receiving comms runtime");
        let sender_comms = service
            .comms_runtime(&sender)
            .await
            .expect("sending comms runtime");
        let receiver_descriptor = descriptor(&receiver_comms);
        let sender_descriptor = descriptor(&sender_comms);
        receiver_peer
            .set(receiver_descriptor.peer_id)
            .expect("the receiving peer is recorded once");
        machine
            .stage_add_direct_peer_endpoint(
                &sender,
                meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&receiver_descriptor),
                Arc::clone(&sender_comms),
            )
            .await
            .expect("the sending agent trusts the receiving agent");
        machine
            .stage_add_direct_peer_endpoint(
                &receiver,
                meerkat_runtime::meerkat_machine::dsl::PeerEndpoint::from(&sender_descriptor),
                Arc::clone(&receiver_comms),
            )
            .await
            .expect("the receiving agent trusts the sending agent");

        // Both agents' controller credential, through the machine's own
        // generated credential owner, as a host that acquired it supplies it.
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &machine.generated_auth_lease_handle(),
            selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-governed-peers-credential"),
            tokio::time::Instant::now() + BOUND,
        )
        .await
        .expect("fixture credential before native admission");

        Self {
            machine,
            host,
            transport,
            sender,
            receiver,
            sender_peer: sender_descriptor.peer_id,
            receiver_peer: receiver_descriptor.peer_id,
            sender_controller,
            sender_basis,
        }
    }

    /// The requester's original on the sending agent: requester, ingress
    /// and the requester's delegation, in the host's ingress domain.
    pub fn sender_association(&self, pin: &ControllerModelClient) -> InputAuthorityAssociation {
        InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: requester(),
            ingress_actor: ingress(),
            represented_subject: None,
            original_authentication: evidence("requester-authentication"),
            logical_executor: sender_agent(),
            target: self
                .host
                .target(&LogicalRuntimeId::for_session(&self.sender)),
            original_work: OriginalWorkRef {
                authority: ingress(),
                work: id("requester-question"),
            },
            root_event: evidence("requester-question-event"),
            contributing_work: Vec::new(),
            authority_basis: WorkAuthorityBasis::GrantLineage {
                lineage: vec![self.sender_basis.clone()],
            },
            controller_grant_lineage: vec![self.sender_controller.clone()],
            controller_model: Some(pin.selection().clone()),
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: Vec::new(),
            ingress_namespace: QualifiedIngressNamespace {
                realm: realm(),
                ingress: ingress_domain(),
                occurrence_scope: id("requester-question-occurrence"),
            },
            contract: ContractRequirements::local_governed_v1(),
        })
        .expect("the requester's original claims")
    }

    /// The requester asks the sending agent, through the host's ingress, and
    /// the sending agent's governed run ends. The run's one delivery to the
    /// receiving agent has then been decided at the receiver: an in-process
    /// send returns once the receiver's admission answered it.
    pub async fn ask(&self) {
        let actor = self
            .host
            .service
            .live_session_actor_witness(&self.sender)
            .await
            .expect("live sending agent");
        let pin = self
            .host
            .service
            .pin_controller_client_for_actor(&actor)
            .await
            .expect("the sending agent's controller client");
        let mut prompt = PromptInput::new(SENDER_PROMPT, None);
        prompt.header.authority_association = Some(self.sender_association(&pin));
        let input = Input::Prompt(prompt);
        let current = NativeIngressContext::from_trusted_ingress(
            &input,
            requester(),
            ingress(),
            realm(),
            evidence("requester-transport-authentication"),
        )
        .expect("trusted ingress observation")
        .with_controller_client(&input, pin)
        .expect("selected client bound before admission");
        let input = input
            .with_ingress_context(current)
            .expect("exact final input");
        let (_, completion) = self
            .machine
            .accept_input_with_completion(&self.sender, input)
            .await
            .expect("the requester's question is admitted");
        let outcome = tokio::time::timeout(BOUND, completion.expect("completion waiter").wait())
            .await
            .expect("the sending agent's run ends within bound");
        assert!(
            matches!(outcome, Ok(CompletionOutcome::Completed(_))),
            "the sending agent's run completed: {outcome:?}"
        );
    }

    /// The authority association of every input `session` admitted, by input
    /// id: each row's own original.
    pub async fn admitted(
        &self,
        session: &SessionId,
    ) -> Vec<(String, InputAuthorityAssociationCandidate)> {
        let mut rows: Vec<_> = self
            .machine
            .session_input_states(session)
            .await
            .expect("input census")
            .report
            .into_iter()
            .filter_map(|row| {
                let own = row
                    .state
                    .authority_contributors
                    .iter()
                    .find(|contributor| contributor.input_id() == &row.state.input_id)?
                    .association()
                    .candidate()
                    .clone();
                Some((row.state.input_id.to_string(), own))
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    /// The number of inputs `session` holds, whatever their attribution.
    pub async fn input_count(&self, session: &SessionId) -> usize {
        self.machine
            .session_input_states(session)
            .await
            .expect("input census")
            .report
            .len()
    }
}
