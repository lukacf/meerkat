//! Approval branches: an approved work runs in a one-time branch of its
//! grantee, a member fork made at an empty prefix, and resolves its approval
//! grant with the branch as executor.
//!
//! One persistent mob on a governed machine with the real generated grant
//! owner. The grantee (`data-agent`) first answers a question of its own, so
//! its transcript is not empty. The branch is forked from it at prefix 0 and
//! seated as a member of its own. The approved work names the branch as its
//! executor; its basis selects one target, the peer directory, whose lineage
//! ends at a child grant issued to the branch (the approval grant), and no
//! base; its controller lineage ends at a controller grant the grantee issued
//! to the branch. Its one tool call (`peers`) must resolve the approval
//! grant.
//!
//! Rows:
//! - the quiescent case;
//! - the grantee mid-turn while the branch is forked;
//! - the grantee's own controller lineage does not serve its branch (so the
//!   branch runs under a controller grant of its own);
//! - a control: the approval grant revoked, the operation is refused.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod support;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt as _;
use meerkat_authorization::clock::HostAuthorizationClock;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, ControllerAdmissionAllowance, OperationPolicyOwner,
    WorkOwnerAllowance, single_tool_operation_target,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, DelegationDepth, DomainRestriction, ExactRestriction,
    ExecutionRestrictions, LifetimeRestriction, ProcessorRef, ResourceDomain,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, GrantTarget, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
    NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace, TargetGrantLineage,
    WorkAuthorityBasis,
};
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse,
    OperationObservedOutcome, OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
    ToolAuthorizationTarget,
};
use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
use meerkat_core::{
    ControllerModelClient, ControllerModelSelection, Message, PrincipalKind, PrincipalRef,
    Provider, SessionId, SessionLlmIdentity, StopReason, ToolOperationTarget, ToolResult,
    TrustDomainId,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PreparedLlmRequest,
};
use meerkat_mob::definition::WiringRules;
use meerkat_mob::{
    AgentIdentity, MemberTurnHandle, MobBuilder, MobDefinition, MobError, MobHandle, MobId,
    MobRuntimeMode, MobSessionService, MobStorage, Profile, ProfileBinding, ProfileName,
    SpawnMemberSpec, ToolConfig,
};
use meerkat_runtime::identifiers::{IdempotencyKey, LogicalRuntimeId};
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_runtime::meerkat_machine::{
    MeerkatMachine, NativeGrantWorkConfiguration, NativeIngressCheck, bind_scoped_original,
};

const MODEL: &str = "claude-sonnet-4-5";
const ENDPOINT: &str = "https://approval-branch.invalid/messages";
const REALM: &str = "approval-branch";
/// The requester's ordinary question to the grantee.
const GRANTEE_PROMPT: &str = "Which sources can you read for me?";
const GRANTEE_ANSWER: &str = "the grantee's own answer";
/// The approved work's input.
const APPROVED_PROMPT: &str = "Approved access: run exactly this operation once";
const APPROVED_CALL: &str = "approved-operation";
const BRANCH_ANSWER: &str = "the approved operation ran";
const DECISION: &str = "decision-1";
const BOUND: Duration = Duration::from_mins(1);

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded fixture identifier")
}

fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new(REALM).expect("domain"),
    )
    .expect("qualified fixture principal")
}

/// The person who asked.
fn requester() -> PrincipalRef {
    principal("requester")
}

/// The host's ingress that authenticated the requester's own question.
fn host_ingress() -> PrincipalRef {
    principal("host-ingress")
}

/// The runtime's decision ingress: the approved work's ingress actor.
fn decision_ingress() -> PrincipalRef {
    principal("decision-ingress")
}

/// The grantee: the agent the requester asked.
fn data_agent() -> PrincipalRef {
    principal("data-agent")
}

/// The parent grant's grantee: the agent that offered the access.
fn owner_agent() -> PrincipalRef {
    principal("owner-agent")
}

/// The approval branch's executor, derived from the grantee and the decision.
fn branch_executor() -> PrincipalRef {
    principal(&format!("approval-branch-of-data-agent-{DECISION}"))
}

fn grant_root() -> PrincipalRef {
    principal("grant-owner")
}

fn realm() -> RealmId {
    RealmId::parse(REALM).expect("realm")
}

fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: host_ingress(),
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
    serde_json::from_value(serde_json::json!({"realm": REALM, "account": "scripted-controller"}))
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

fn action(verb: &str) -> ActionRef {
    ActionRef {
        feature: "approval-branch".into(),
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

/// Bounds over one verb in the fixture's domain, every other dimension open.
fn verb_bounds(verb: &str) -> ExecutionRestrictions {
    let mut bounds = ExecutionRestrictions::unrestricted();
    bounds.actions = ExactRestriction::exact([action(verb)]);
    bounds.resource_domains = DomainRestriction::exact([domain()]);
    bounds
}

/// The grant owner's grants for one fixture.
struct Grants {
    owner: Arc<LocalGrantAuthority>,
    /// The grantee's controller grant (unrestricted lifetime, delegable).
    grantee_controller: GrantLineageRef,
    /// The parent of the access offer: the owner's delegation to its agent.
    owner_delegation: GrantLineageRef,
}

/// A grant custody that vetoes nothing: no grant revoked here is a
/// controller lineage in use.
struct AnyCustody;

impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
    for AnyCustody
{
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _reference: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<
        Result<T, E>,
        meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal,
    > {
        Ok(mutate())
    }
}

impl Grants {
    fn new() -> Self {
        let owner = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: grant_root(),
                    namespace: id("approval-branch-grants"),
                    generation: 1,
                },
                LocalAuthorizationPublication::new(),
                Arc::new(HostAuthorizationClock),
            )
            .expect("generated grant owner"),
        );
        let grantee_controller = owner
            .issue_root(
                &grant_root(),
                id("grantee-controller"),
                data_agent(),
                None,
                verb_bounds("infer"),
            )
            .expect("the grantee's controller grant");
        let mut delegation = verb_bounds("read-directory");
        delegation.delegation_depth = DelegationDepth::remaining(1);
        let owner_delegation = owner
            .issue_root(
                &grant_root(),
                id("owner-delegation"),
                owner_agent(),
                None,
                delegation,
            )
            .expect("the owner's delegation to its agent");
        Self {
            owner,
            grantee_controller,
            owner_delegation,
        }
    }

    /// The approval grant: the owner's agent issues the approved terms (one
    /// operation verb, the requester as audience, a 15-minute window, no
    /// further delegation) to `executor`. The root-to-leaf lineage.
    fn approval_to(&self, executor: &PrincipalRef) -> Vec<GrantLineageRef> {
        let now_ms = now_ms();
        let mut terms = verb_bounds("read-directory");
        terms.processors = ExactRestriction::exact([ProcessorRef::Principal {
            principal: executor.clone(),
        }]);
        terms.audiences = ExactRestriction::exact([AudienceRef::Principal {
            principal: requester(),
        }]);
        terms.lifetime = LifetimeRestriction::window(now_ms, now_ms + 15 * 60 * 1000);
        terms.delegation_depth = DelegationDepth::remaining(0);
        let approval = self
            .owner
            .issue_child(
                &owner_agent(),
                &self.owner_delegation,
                id(&format!("approval-{DECISION}")),
                executor.clone(),
                terms,
            )
            .expect("the approval grant");
        vec![self.owner_delegation.clone(), approval]
    }

    /// A controller lineage for the branch: a child of the grantee's
    /// controller grant, issued by the grantee to the branch.
    fn branch_controller(&self) -> Vec<GrantLineageRef> {
        let mut requested = verb_bounds("infer");
        requested.processors = ExactRestriction::exact([ProcessorRef::Principal {
            principal: branch_executor(),
        }]);
        requested.delegation_depth = DelegationDepth::remaining(0);
        let child = self
            .owner
            .issue_child(
                &data_agent(),
                &self.grantee_controller,
                id(&format!("branch-controller-{DECISION}")),
                branch_executor(),
                requested,
            )
            .expect("the branch's controller child");
        vec![self.grantee_controller.clone(), child]
    }

    /// The owner's agent, the approval grant's issuer, revokes it.
    fn revoke(&self, lineage: &[GrantLineageRef]) {
        let leaf = lineage.last().expect("a lineage");
        self.owner
            .revoke(&owner_agent(), leaf, &mut AnyCustody)
            .expect("the issuer revokes the approval grant");
    }
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_millis(),
    )
    .expect("milliseconds fit")
}

/// The host's application entitlement: the requester's own question to the
/// grantee, and the approved work in the branch.
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
        let grantee_work =
            claims.ingress_actor == host_ingress() && claims.logical_executor == data_agent();
        let approved_work = claims.ingress_actor == decision_ingress()
            && claims.logical_executor == branch_executor();
        if claims.requester != requester() || !(grantee_work || approved_work) {
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

/// Maps the controller's inference and one tool: the peer directory read.
struct Operations;

impl OperationPolicyOwner for Operations {
    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        _: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        if facts.selection() != &selection()
            || facts.endpoint() != ENDPOINT
            || facts.wire_model() != MODEL
        {
            return Err(denied().into());
        }
        Ok(ControllerAdmissionAllowance {
            operation_values: vec![values("infer", &association.candidate().logical_executor)],
            restrictions: ExecutionRestrictions::unrestricted(),
        })
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
                if purpose == LocalPolicyPurpose::Operation && facts.name == "peers" =>
            {
                let ToolAuthorizationTarget::Dispatcher(plan) = &facts.target else {
                    return Err(denied().into());
                };
                match single_tool_operation_target(plan)? {
                    Some(ToolOperationTarget::PeerDirectoryRead) => "read-directory",
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

/// The host's custody of originals: what each original's admission is
/// authenticated against. First writer wins per original work.
#[derive(Default)]
struct Originals(Mutex<HashMap<String, InputAuthorityAssociation>>);

impl Originals {
    fn enroll(&self, association: &InputAuthorityAssociation) {
        let work = association
            .candidate()
            .original_work
            .work
            .as_str()
            .to_owned();
        let mut enrolled = self.0.lock().expect("originals");
        match enrolled.get(&work) {
            Some(first) => assert!(first == association, "a differing enrollment of {work}"),
            None => {
                enrolled.insert(work, association.clone());
            }
        }
    }

    fn admits(&self, runtime: &LogicalRuntimeId, claimed: &InputAuthorityAssociation) -> bool {
        let claims = claimed.candidate();
        claims.target.logical_runtime == id(&runtime.to_string())
            && self
                .0
                .lock()
                .expect("originals")
                .get(claims.original_work.work.as_str())
                .is_some_and(|enrolled| enrolled == claimed)
    }
}

/// Every session's controller transport. The grantee's turn on its own
/// question answers text (held while `hold_grantee` is set); the approved
/// work calls `peers` once, then answers.
struct ScriptedController {
    grantee_released: tokio::sync::watch::Sender<bool>,
    grantee_entered: AtomicUsize,
    /// The messages of every request the approved work made.
    approved_requests: Mutex<Vec<Vec<Message>>>,
    approved_feedback: Mutex<Vec<ToolResult>>,
}

impl Default for ScriptedController {
    fn default() -> Self {
        Self {
            grantee_released: tokio::sync::watch::channel(true).0,
            grantee_entered: AtomicUsize::new(0),
            approved_requests: Mutex::new(Vec::new()),
            approved_feedback: Mutex::new(Vec::new()),
        }
    }
}

fn mentions(messages: &[Message], needle: &str) -> bool {
    messages.iter().any(|message| {
        serde_json::to_string(message)
            .expect("message renders")
            .contains(needle)
    })
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

fn text(reply: &'static str) -> LlmStream<'static> {
    Box::pin(futures::stream::iter(vec![
        Ok(LlmEvent::TextDelta {
            delta: reply.into(),
            meta: None,
        }),
        Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::success(StopReason::EndTurn),
        }),
    ]))
}

#[async_trait]
impl LlmClient for ScriptedController {
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
        let messages = &request.request().messages;
        if mentions(messages, APPROVED_PROMPT) {
            self.approved_requests
                .lock()
                .expect("requests")
                .push(messages.clone());
            if let Some(feedback) = tool_feedback(messages, APPROVED_CALL) {
                self.approved_feedback
                    .lock()
                    .expect("feedback")
                    .push(feedback);
                return text(BRANCH_ANSWER);
            }
            return Box::pin(futures::stream::iter(vec![
                Ok(LlmEvent::ToolCallComplete {
                    id: APPROVED_CALL.into(),
                    name: "peers".into(),
                    args: serde_json::json!({}),
                    meta: None,
                }),
                Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                }),
            ]));
        }
        if mentions(messages, GRANTEE_PROMPT) {
            self.grantee_entered.fetch_add(1, Ordering::SeqCst);
            let mut released = self.grantee_released.subscribe();
            return Box::pin(
                futures::stream::once(async move {
                    while !*released.borrow_and_update() {
                        if released.changed().await.is_err() {
                            break;
                        }
                    }
                    Ok(LlmEvent::TextDelta {
                        delta: GRANTEE_ANSWER.into(),
                        meta: None,
                    })
                })
                .chain(futures::stream::iter([Ok(LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::EndTurn),
                })])),
            );
        }
        text("unexpected request")
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

fn mob_definition() -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: MODEL.to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "governed data agent".to_string(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "approval-branch-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![],
    };
    definition
}

/// One persistent governed mob with the grantee seated.
struct Fixture {
    _realm: Arc<support::MemberRealm>,
    handle: MobHandle,
    _machine: Arc<MeerkatMachine>,
    service: Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
    controller: Arc<ScriptedController>,
    originals: Arc<Originals>,
    grants: Grants,
    grantee: AgentIdentity,
    grantee_session: SessionId,
}

impl Fixture {
    async fn new() -> Self {
        let realm = support::member_realm();
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::default());
        let controller = Arc::new(ScriptedController::default());
        let service = support::persistent_service_with_client_and_blob_store(
            &realm.paths,
            Arc::clone(&realm.runtime_store),
            controller.clone(),
            Arc::clone(&blob_store),
        );
        let grants = Grants::new();
        let originals = Arc::new(Originals::default());
        let custody = Arc::clone(&originals);
        let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
            let claims = claimed.candidate();
            if current.requester() != &requester()
                || claims.controller_model.as_ref() != Some(&selection())
                || !custody.admits(runtime, claimed)
            {
                return Err(denied().into());
            }
            Ok(())
        });
        let machine = Arc::new(
            MeerkatMachine::persistent_with_local_grant_authorization(
                Arc::clone(&realm.runtime_store),
                Some(Arc::clone(&blob_store)),
                NativeGrantWorkConfiguration {
                    retained_resume: None,
                    kept_original: None,
                    require_scoped_destination: false,
                    grants: Arc::clone(&grants.owner),
                    ingress,
                    invocation_owner: Arc::new(Invocation),
                    operation_owner: Arc::new(Operations),
                    configured_mcp_attachment: None,
                },
            )
            .expect("governed persistent machine"),
        );
        let adopted = service
            .acquire_canonical_runtime_adapter(Some(Arc::clone(&machine)))
            .expect("the service adopts the governed machine");
        assert!(Arc::ptr_eq(&adopted, &machine));
        let mob_service: Arc<dyn MobSessionService> = service.clone();
        let handle = MobBuilder::new(mob_definition(), MobStorage::in_memory())
            .with_session_service(mob_service)
            .with_runtime_adapter(Arc::clone(&machine))
            .create()
            .await
            .expect("a persistent mob over the governed machine");
        let grantee = AgentIdentity::from("data-agent");
        handle
            .spawn_spec(SpawnMemberSpec::new("worker", grantee.clone()))
            .await
            .expect("spawn the grantee");
        let grantee_session = handle
            .resolve_bridge_session_id(&grantee)
            .await
            .expect("the grantee's session");
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &machine.generated_auth_lease_handle(),
            selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-approval-branch-credential"),
            tokio::time::Instant::now() + BOUND,
        )
        .await
        .expect("fixture credential before native admission");
        Self {
            _realm: realm,
            handle,
            _machine: machine,
            service,
            controller,
            originals,
            grants,
            grantee,
            grantee_session,
        }
    }

    /// The session's live controller client, pinned without queueing behind
    /// a turn: a held turn boundary (a just-seated member's startup lap) is
    /// tried again until it is free.
    async fn pin(&self, session: &SessionId) -> ControllerModelClient {
        let actor = self
            .service
            .live_session_actor_witness(session)
            .await
            .expect("a live session");
        tokio::time::timeout(BOUND, async {
            loop {
                match self
                    .service
                    .try_pin_controller_client_for_actor(&actor)
                    .await
                {
                    Ok(pin) => return pin,
                    Err(meerkat_session::TryPinControllerError::TurnBoundaryHeld) => {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    Err(error) => panic!("the session's controller client: {error:?}"),
                }
            }
        })
        .await
        .expect("the session's turn boundary frees within bound")
    }

    fn target(session: &SessionId) -> NativeWorkTarget {
        NativeWorkTarget {
            logical_owner: principal("native-owner"),
            logical_runtime: id(&LogicalRuntimeId::for_session(session).to_string()),
            context: id("native-context"),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: requester(),
            },
        }
    }

    fn namespace(ingress: PrincipalRef, occurrence: &str) -> QualifiedIngressNamespace {
        QualifiedIngressNamespace {
            realm: realm(),
            ingress: ResourceDomain {
                authority: ingress,
                namespace: "works".into(),
            },
            occurrence_scope: id(occurrence),
        }
    }

    /// The requester's own question `n` to the grantee: controller-only work.
    fn grantee_work(&self, pin: &ControllerModelClient, n: usize) -> InputAuthorityAssociation {
        InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: requester(),
            ingress_actor: host_ingress(),
            represented_subject: None,
            original_authentication: evidence("requester-authentication"),
            logical_executor: data_agent(),
            target: Self::target(&self.grantee_session),
            original_work: OriginalWorkRef {
                authority: host_ingress(),
                work: id(&format!("requester-question-{n}")),
            },
            root_event: evidence(&format!("requester-question-{n}-event")),
            contributing_work: Vec::new(),
            authority_basis: WorkAuthorityBasis::TargetedGrantLineages {
                base: None,
                targets: Vec::new(),
            },
            controller_grant_lineage: vec![self.grants.grantee_controller.clone()],
            controller_model: Some(pin.selection().clone()),
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: Vec::new(),
            ingress_namespace: Self::namespace(
                host_ingress(),
                &format!("requester-question-{n}-occurrence"),
            ),
            contract: ContractRequirements::local_governed_v1(),
        })
        .expect("the grantee work's claims")
    }

    /// The approved work in `branch`: requester kept, the branch as
    /// executor, one target lineage ending at the approval grant, no base.
    fn approved_work(
        &self,
        branch: &SessionId,
        pin: &ControllerModelClient,
        executor: PrincipalRef,
        operation_lineage: Vec<GrantLineageRef>,
        controller_lineage: Vec<GrantLineageRef>,
    ) -> InputAuthorityAssociation {
        InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: requester(),
            ingress_actor: decision_ingress(),
            represented_subject: None,
            original_authentication: evidence(&format!("{DECISION}-executed")),
            logical_executor: executor,
            target: Self::target(branch),
            original_work: OriginalWorkRef {
                authority: decision_ingress(),
                work: id(&format!("approved-work-{DECISION}")),
            },
            root_event: evidence(&format!("{DECISION}-approved")),
            contributing_work: Vec::new(),
            authority_basis: WorkAuthorityBasis::TargetedGrantLineages {
                base: None,
                targets: vec![TargetGrantLineage {
                    target: GrantTarget::PeerDirectory,
                    lineage: operation_lineage,
                }],
            },
            controller_grant_lineage: controller_lineage,
            controller_model: Some(pin.selection().clone()),
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: Vec::new(),
            ingress_namespace: Self::namespace(
                decision_ingress(),
                &format!("approved-work-{DECISION}-occurrence"),
            ),
            contract: ContractRequirements::local_governed_v1(),
        })
        .expect("the approved work's claims")
    }

    /// Enroll `association` and bind `text` under it to its final input with
    /// the original event's key, as the trusted ingress `ingress` observed it.
    fn bound(
        &self,
        text: &str,
        ingress: PrincipalRef,
        association: InputAuthorityAssociation,
        pin: ControllerModelClient,
    ) -> Input {
        self.originals.enroll(&association);
        let event_key = association
            .candidate()
            .original_work
            .work
            .as_str()
            .to_owned();
        bind_scoped_original(
            Input::Prompt(PromptInput::new(text, None)),
            association,
            IdempotencyKey::new(event_key),
            requester(),
            ingress,
            realm(),
            evidence("transport-authentication"),
            pin,
        )
        .expect("the final input is bound to its trusted ingress observation")
    }

    /// The requester asks the grantee its own question `n`: external work.
    async fn ask_grantee(&self, n: usize) -> MemberTurnHandle {
        let pin = self.pin(&self.grantee_session).await;
        let association = self.grantee_work(&pin, n);
        let input = self.bound(GRANTEE_PROMPT, host_ingress(), association, pin);
        self.handle
            .start_governed_input(&self.grantee, input)
            .await
            .expect("the grantee's own work is admitted")
    }

    /// The approved work, delivered by the trusted host composition to the
    /// branch it composed: internal work.
    async fn run_approved_work(
        &self,
        branch: &meerkat_mob::ForkMemberResult,
        executor: PrincipalRef,
        operation_lineage: Vec<GrantLineageRef>,
        controller_lineage: Vec<GrantLineageRef>,
    ) -> Result<MemberTurnHandle, MobError> {
        let pin = self.pin(&branch.session_id).await;
        let association = self.approved_work(
            &branch.session_id,
            &pin,
            executor,
            operation_lineage,
            controller_lineage,
        );
        let input = self.bound(APPROVED_PROMPT, decision_ingress(), association, pin);
        self.handle
            .start_internal_governed_input_with_custody(
                &branch.agent_identity,
                input,
                Box::new(NoCustody),
            )
            .await
    }

    /// The approval branch: the grantee forked at an empty prefix.
    async fn fork_branch(&self) -> Result<meerkat_mob::ForkMemberResult, MobError> {
        self.handle
            .fork_member_at_empty_prefix(
                &self.grantee,
                SpawnMemberSpec::new("worker", AgentIdentity::from(format!("branch-{DECISION}"))),
            )
            .await
    }
}

/// The approved work's resources need no custody in this composition.
struct NoCustody;

impl meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody for NoCustody {
    fn settle(
        self: Box<Self>,
        _: meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement,
    ) -> Option<meerkat_runtime::input_admission_custody::NativeInputAdmissionCompletion> {
        None
    }
}

async fn completed(turn: MemberTurnHandle, what: &str) {
    let outcome = tokio::time::timeout(BOUND, turn.wait())
        .await
        .unwrap_or_else(|_| panic!("{what} ends within bound"));
    assert!(outcome.is_ok(), "{what} completed: {outcome:?}");
}

/// The approved work's single tool call resolved the approval grant: it
/// was entered (no refusal) and answered.
fn assert_the_approved_operation_ran(controller: &ScriptedController) {
    let feedback = controller
        .approved_feedback
        .lock()
        .expect("feedback")
        .clone();
    let [result] = feedback.as_slice() else {
        panic!("one result for the approved operation: {feedback:?}");
    };
    assert!(
        !result.is_error,
        "the approved operation ran under the approval grant: {}",
        result.text_content()
    );
    assert!(
        !result.text_content().contains("operation_refused"),
        "the approved operation was not refused: {}",
        result.text_content()
    );
}

/// The branch's transcript starts empty: none of the approved work's model
/// requests carries anything of the grantee's own conversation.
fn assert_branch_saw_only_the_approved_work(controller: &ScriptedController) {
    let requests = controller
        .approved_requests
        .lock()
        .expect("requests")
        .clone();
    assert!(!requests.is_empty(), "the approved work reached the model");
    for messages in &requests {
        assert!(
            !mentions(messages, GRANTEE_PROMPT) && !mentions(messages, GRANTEE_ANSWER),
            "the branch holds nothing of the grantee's conversation: {messages:?}"
        );
    }
}

/// Quiescent grantee: the branch is forked at prefix 0 after the grantee
/// answered its own question; the approved work is admitted there with the
/// branch as executor and its one operation resolves the approval grant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approved_work_runs_in_an_empty_prefix_branch_of_a_quiescent_grantee() {
    let fixture = Fixture::new().await;
    completed(fixture.ask_grantee(1).await, "the grantee's own question").await;

    let branch = fixture
        .fork_branch()
        .await
        .expect("the grantee forks at an empty prefix");
    let approval = fixture.grants.approval_to(&branch_executor());
    let turn = fixture
        .run_approved_work(
            &branch,
            branch_executor(),
            approval,
            fixture.grants.branch_controller(),
        )
        .await
        .expect("the approved work is admitted into the branch");
    completed(turn, "the approved work").await;

    assert_the_approved_operation_ran(&fixture.controller);
    assert_branch_saw_only_the_approved_work(&fixture.controller);
}

/// The grantee is mid-turn (its own question's model request is held) when
/// the branch is forked at prefix 0. The empty prefix needs nothing of that
/// turn: the fork is made, the approved work runs in the branch, and the
/// grantee's turn then completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approved_work_runs_in_an_empty_prefix_branch_of_a_grantee_mid_turn() {
    let fixture = Fixture::new().await;
    // A first question, answered, so the grantee's transcript is not empty.
    completed(fixture.ask_grantee(1).await, "the grantee's first question").await;
    fixture.controller.grantee_released.send_replace(false);
    let entered = fixture.controller.grantee_entered.load(Ordering::SeqCst);
    let grantee_turn = fixture.ask_grantee(2).await;
    tokio::time::timeout(BOUND, async {
        while fixture.controller.grantee_entered.load(Ordering::SeqCst) == entered {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the grantee's second turn is at its model request");

    let branch = fixture
        .fork_branch()
        .await
        .expect("a grantee mid-turn forks at an empty prefix");
    let approval = fixture.grants.approval_to(&branch_executor());
    let turn = fixture
        .run_approved_work(
            &branch,
            branch_executor(),
            approval,
            fixture.grants.branch_controller(),
        )
        .await
        .expect("the approved work is admitted into the branch");
    completed(turn, "the approved work").await;
    assert_the_approved_operation_ran(&fixture.controller);
    assert_branch_saw_only_the_approved_work(&fixture.controller);
    assert_eq!(
        fixture.controller.grantee_entered.load(Ordering::SeqCst),
        entered + 1,
        "the grantee's second turn was still held while the branch ran"
    );

    fixture.controller.grantee_released.send_replace(true);
    completed(grantee_turn, "the grantee's held turn").await;
}

/// The grantee's own controller lineage does not serve its branch: the
/// grant owner resolves a controller lineage only for its leaf's grantee, so
/// with the branch as executor the grantee's own lineage is refused at
/// admission. The branch runs under a controller grant of its own, a child
/// of the grantee's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_grantees_own_controller_lineage_does_not_resolve_for_the_branch() {
    let fixture = Fixture::new().await;
    completed(fixture.ask_grantee(1).await, "the grantee's own question").await;
    let branch = fixture
        .fork_branch()
        .await
        .expect("the grantee forks at an empty prefix");
    let approval = fixture.grants.approval_to(&branch_executor());
    let admitted = fixture
        .run_approved_work(
            &branch,
            branch_executor(),
            approval,
            vec![fixture.grants.grantee_controller.clone()],
        )
        .await;
    match admitted {
        Err(MobError::MemberInputRefused { error, .. }) => {
            eprintln!("approved work refused at admission: {error:?}");
        }
        Err(other) => panic!("refused, but not by native admission: {other:?}"),
        Ok(turn) => {
            let outcome = tokio::time::timeout(BOUND, turn.wait()).await;
            panic!("the grantee's own controller lineage served the branch, outcome {outcome:?}");
        }
    }
    assert!(
        fixture
            .controller
            .approved_requests
            .lock()
            .expect("requests")
            .is_empty(),
        "nothing of the approved work reached the model"
    );
}

/// Control: the operation's authority is the approval grant. Revoked
/// before the approved work runs, the same operation is refused before
/// entry, while the work itself is still admitted on its controller.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_approval_grant_refuses_the_approved_operation() {
    let fixture = Fixture::new().await;
    completed(fixture.ask_grantee(1).await, "the grantee's own question").await;
    let branch = fixture
        .fork_branch()
        .await
        .expect("the grantee forks at an empty prefix");
    let approval = fixture.grants.approval_to(&branch_executor());
    fixture.grants.revoke(&approval);
    let turn = fixture
        .run_approved_work(
            &branch,
            branch_executor(),
            approval,
            fixture.grants.branch_controller(),
        )
        .await
        .expect("the approved work is admitted on its controller lineage");
    completed(turn, "the approved work").await;
    let feedback = fixture
        .controller
        .approved_feedback
        .lock()
        .expect("feedback")
        .clone();
    let [result] = feedback.as_slice() else {
        panic!("one result for the approved operation: {feedback:?}");
    };
    assert!(
        result.is_error && result.text_content().contains("operation_refused"),
        "the operation was refused without its grant: {}",
        result.text_content()
    );
}
