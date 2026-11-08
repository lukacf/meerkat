//! Approval-carried governed child grants, end to end in one governed mob.
//!
//! The requester asks the owner's agent (`owner-agent`) for data of a
//! governed source the agent reaches under the owner's delegation, whose
//! children each need the owner's decision (a child rule for the approver id
//! `data-owner`). The agent's model calls `access_offer` in that governed
//! work: Core holds a `grant.child` decision. A person approves it; the
//! executor loop opens a one-time approval branch of the agent (a fork at an
//! empty prefix, labelled with the decision), issues the branch its
//! controller grant and the approval-carried grant, and admits the approved
//! work into the branch. There the model runs the exact query once (a second
//! run is refused: its one use is spent), replies to the requester once
//! (`reply_to_requester`), and the branch is retired when the work ends.
//!
//! One persistent mob on a governed machine with the real generated grant
//! owner, the real decision owner and the native admission, host owners as
//! fixtures. The source is a host tool that states the exact MCP operation
//! and its request digest, as the MCP router does.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod support;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_authorization::clock::HostAuthorizationClock;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, ControllerAdmissionAllowance, OperationPolicyOwner,
    WorkOwnerAllowance, single_tool_operation_target,
};
use meerkat_authorization::grants::{
    ChildApprovalRule, GrantDeviceTrust, LocalGrantAuthority, LocalGrantConfiguration,
};
use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, DelegationDepth, ExecutionRestrictions, ProcessorRef, ResourceDomain,
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
use meerkat_core::approval::consent::{
    AuthenticatedApprover, ConsentEligibility, ConsentEligibilityAnswer, ConsentIneligible,
};
use meerkat_core::approval::decision::{
    ApproverId, AttestedDecision, DecisionId, DecisionIngress, DecisionListFilter, DecisionPhase,
    DecisionReference, DecisionStore, InMemoryDecisionStore, PresentationAudience,
    PresentationRecord, StoredDecision,
};
use meerkat_core::approval::{ApprovalAssurance, ApprovalConsentDeliveryState};
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse,
    OperationObservedOutcome, OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
    ToolAuthorizationTarget,
};
use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
use meerkat_core::types::{ToolCallView, ToolDef, ToolResult};
use meerkat_core::{
    AgentToolDispatcher, ControllerModelClient, ControllerModelSelection, Message, PrincipalKind,
    PrincipalRef, Provider, SessionId, SessionLlmIdentity, StopReason, ToolDispatchContext,
    ToolDispatchOutcome, ToolOperationTarget, TrustDomainId,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PreparedLlmRequest,
};
use meerkat_mob::definition::WiringRules;
use meerkat_mob::{
    AgentIdentity, MemberTurnHandle, MobBuilder, MobDefinition, MobHandle, MobId, MobRuntimeMode,
    MobSessionService, MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec,
    ToolConfig,
};
use meerkat_runtime::approval_grants::{
    APPROVAL_BRANCH_LABEL, AccessOutcome, ApprovalBranchError, ApprovalBranchHost,
    ApprovalBranchRequest, ApprovalBranchSeat, ApprovalControllerCustody, ApprovalGranteeBasis,
    ApprovalGrants, ApprovalGrantsConfiguration, ApprovalOrigin, ApprovedWorkAdmission,
    DecisionApprovalUseCurrentness, GrantChildRequest, ReplyReceipt, ReplyRouteError,
    RequesterReplyRoute, approval_branch_principal, approval_grant_id, approved_work_id,
    branch_controller_grant_id,
};
use meerkat_runtime::decision::{
    DecisionApproverCurrentness, DecisionExecutorLoop, DecisionPassOutcome, DurableDecisionService,
};
use meerkat_runtime::identifiers::{IdempotencyKey, LogicalRuntimeId};
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_runtime::input_admission_custody::NativeInputAdmissionCustody;
use meerkat_runtime::meerkat_machine::{
    MeerkatMachine, NativeGrantWorkConfiguration, NativeIngressCheck, bind_scoped_original,
};

const MODEL: &str = "claude-sonnet-4-5";
const ENDPOINT: &str = "https://approval-grants.invalid/messages";
const REALM: &str = "approval-grants";
const SERVER: &str = "records";
const TOOL: &str = "query";
const SOURCE_TOOL: &str = "records_query";
/// The requester's question to the owner's agent.
const OWNER_PROMPT: &str = "The requester asks for last quarter's revenue by region.";
const OWNER_ANSWER: &str = "I asked the data owner to approve this exact query.";
const SECOND_QUESTION: &str = "Is there anything else you can tell the requester?";
const OFFER_CALL: &str = "offer-call";
/// Every approved work's input starts with it.
const APPROVED_MARKER: &str = "Approved access: run exactly this operation once";
const SOURCE_RESULT: &str = "revenue by region: north 42, south 17";
const REPLY_TEXT: &str = "Last quarter's revenue: north 42, south 17.";
const OTHER_REPLY: &str = "Another answer altogether.";
/// A question whose answer would need a source no grant of the work covers.
const LEDGER_PROMPT: &str = "The requester asks for the ledger's open invoices.";
const LEDGER_OFFER_CALL: &str = "ledger-offer-call";
const LEDGER_ANSWER: &str = "The ledger is not mine to offer.";
const BOUND: Duration = Duration::from_mins(1);

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded fixture identifier")
}

fn principal(kind: PrincipalKind, value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(kind, value, TrustDomainId::new(REALM).expect("domain"))
        .expect("qualified fixture principal")
}

fn agent(value: &str) -> PrincipalRef {
    principal(PrincipalKind::ServiceAccount, value)
}

/// The person who asked.
fn requester() -> PrincipalRef {
    principal(PrincipalKind::Human, "requester")
}

/// The person the owner's agent acts for: the governed source's owner.
fn data_owner() -> PrincipalRef {
    principal(PrincipalKind::Human, "data-owner-person")
}

fn host_ingress() -> PrincipalRef {
    agent("host-ingress")
}

/// The runtime's decision ingress: every approved work's ingress actor.
fn decision_ingress() -> PrincipalRef {
    agent("decision-ingress")
}

fn owner_agent() -> PrincipalRef {
    agent("owner-agent")
}

fn grant_root() -> PrincipalRef {
    agent("grant-root")
}

fn approver() -> ApproverId {
    ApproverId::new("data-owner").expect("approver id")
}

fn realm() -> RealmId {
    RealmId::parse(REALM).expect("realm")
}

fn ingress_domain() -> ResourceDomain {
    ResourceDomain {
        authority: host_ingress(),
        namespace: "works".into(),
    }
}

fn reference(value: &str) -> DecisionReference {
    DecisionReference::new(value).expect("reference")
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

fn query_arguments() -> serde_json::Value {
    serde_json::json!({"query": "revenue by region, last quarter"})
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

fn values(verb: &str, executor: &PrincipalRef) -> LocalOperationValues {
    LocalOperationValues {
        action: ActionRef {
            feature: "approval-grants".into(),
            action: verb.into(),
        },
        resource_domain: ResourceDomain {
            authority: agent("resource-owner"),
            namespace: "records".into(),
        },
        processor: ProcessorRef::Principal {
            principal: executor.clone(),
        },
        audience: AudienceRef::Principal {
            principal: requester(),
        },
    }
}

struct Eligible;

impl ConsentEligibility for Eligible {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}

/// The host's approvers: every person who decided is still current.
struct Approvers;

impl DecisionApproverCurrentness for Approvers {
    fn approver_answer(&self, _decision: &StoredDecision) -> ConsentEligibilityAnswer {
        ConsentEligibilityAnswer::Eligible(Arc::new(Eligible))
    }
}

/// The host's application entitlement: the requester's own question to the
/// owner's agent, and the approved works the decision ingress delivers.
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
        let own_question =
            claims.ingress_actor == host_ingress() && claims.logical_executor == owner_agent();
        let approved_work = claims.ingress_actor == decision_ingress();
        if claims.requester != requester() || !(own_question || approved_work) {
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

/// Maps the controller's inference, a hold of an access offer and the
/// governed source's one query.
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
            AuthorizationOperation::Tool(facts) if purpose == LocalPolicyPurpose::Operation => {
                let ToolAuthorizationTarget::Dispatcher(plan) = &facts.target else {
                    return Err(denied().into());
                };
                match single_tool_operation_target(plan)? {
                    Some(ToolOperationTarget::DecisionHold { .. }) => "hold-offer",
                    Some(ToolOperationTarget::McpServerOperation {
                        server,
                        raw_operation,
                        ..
                    }) if server == SERVER && raw_operation == TOOL => "query",
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
struct Originals {
    enrolled: Mutex<HashMap<String, InputAuthorityAssociation>>,
    /// Every association native admission authenticated, in order.
    admitted: Mutex<Vec<InputAuthorityAssociation>>,
}

impl Originals {
    fn enroll(&self, association: &InputAuthorityAssociation) -> bool {
        let work = association
            .candidate()
            .original_work
            .work
            .as_str()
            .to_owned();
        let mut enrolled = self.enrolled.lock().expect("originals");
        match enrolled.get(&work) {
            Some(first) => first == association,
            None => {
                enrolled.insert(work, association.clone());
                true
            }
        }
    }

    fn admits(&self, runtime: &LogicalRuntimeId, claimed: &InputAuthorityAssociation) -> bool {
        let claims = claimed.candidate();
        let admits = claims.target.logical_runtime == id(&runtime.to_string())
            && self
                .enrolled
                .lock()
                .expect("originals")
                .get(claims.original_work.work.as_str())
                .is_some_and(|enrolled| enrolled == claimed);
        if admits {
            self.admitted
                .lock()
                .expect("admitted")
                .push(claimed.clone());
        }
        admits
    }

    fn enrolled_work(&self, work: &EvidenceId) -> Option<InputAuthorityAssociation> {
        self.enrolled
            .lock()
            .expect("originals")
            .get(work.as_str())
            .cloned()
    }
}

/// The governed source: one tool that states the exact MCP operation it
/// runs and its request's digest, as the MCP router does, and enters its
/// effect once before it answers.
struct Source {
    key: String,
    defs: Arc<[Arc<ToolDef>]>,
    runs: AtomicUsize,
}

impl Source {
    fn new() -> Self {
        Self {
            key: "records-source".into(),
            defs: Arc::from(vec![Arc::new(ToolDef::new(
                SOURCE_TOOL,
                "Run a query of the governed records source.",
                serde_json::json!({
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"]
                }),
            ))]),
            runs: AtomicUsize::new(0),
        }
    }

    fn target() -> ToolOperationTarget {
        ToolOperationTarget::McpServerOperation {
            server: SERVER.into(),
            connection_generation: 1,
            raw_operation: TOOL.into(),
            connector: meerkat_core::McpConnectorApproval::NoConnector,
            account: Arc::new(meerkat_core::OperationAccount::Unavailable),
        }
    }

    fn digest(call: ToolCallView<'_>) -> Option<meerkat_core::approval::decision::Sha256Digest> {
        let arguments: serde_json::Value = serde_json::from_str(call.args.get()).ok()?;
        meerkat_core::mcp_request_digest(SERVER, TOOL, &arguments)
    }
}

#[async_trait]
impl AgentToolDispatcher for Source {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.defs)
    }

    fn review_entry_support(
        &self,
        _tool_name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        meerkat_core::approval::review::ReviewEntrySupport::ConsumesAtEntry
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        _dispatch_context: &ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let digest = Self::digest(call).ok_or_else(|| {
            meerkat_core::ToolExecutionResolutionError::InvalidArguments {
                tool_name: call.name.to_string(),
                reason: "no canonical request".into(),
            }
        })?;
        let witness = meerkat_core::ToolExecutionOwnerWitness::new(
            self.key.clone(),
            call.name,
            self.execution_binding_fingerprint(call.name)?,
        )?
        .with_operation_target(Self::target())
        .with_request_digest(digest);
        meerkat_core::ToolExecutionContract::default()
            .resolve_default(resolution_context.deadlines().clone())?
            .with_owner_witness(witness)
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, meerkat_core::error::ToolError> {
        let (_entering, _custody) = context.enter_reviewed_effect_held(call, Some(plan))?;
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::new(call.id.to_string(), SOURCE_RESULT.to_owned(), false).into())
    }

    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<ToolDispatchOutcome, meerkat_core::error::ToolError> {
        Err(meerkat_core::error::ToolError::access_denied(call.name))
    }
}

/// The approved work's input custody needs no resources here.
struct NoCustody;

impl NativeInputAdmissionCustody for NoCustody {
    fn settle(
        self: Box<Self>,
        _: meerkat_runtime::input_admission_custody::NativeInputAdmissionSettlement,
    ) -> Option<meerkat_runtime::input_admission_custody::NativeInputAdmissionCompletion> {
        None
    }
}

/// What a branch looked like when its approved work was admitted.
#[derive(Debug, Clone)]
struct BranchSnapshot {
    identity: AgentIdentity,
    labels: BTreeMap<String, String>,
    wired_to: Vec<AgentIdentity>,
}

/// The host's approval branches over the mob: the owner's agent is the one
/// grantee it branches; the branch's destination and pinned controller are
/// the fixture's. It records the session of every branch it opened or
/// found.
struct Branches {
    handle: MobHandle,
    service: Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
    originals: Arc<Originals>,
    grantee: AgentIdentity,
    basis: ApprovalGranteeBasis,
    snapshots: Mutex<Vec<BranchSnapshot>>,
    opened_sessions: Mutex<Vec<SessionId>>,
}

impl Branches {
    /// The distinct branch sessions opened or found, in order.
    fn branch_sessions(&self) -> Vec<SessionId> {
        let mut sessions = Vec::new();
        for session in self.opened_sessions.lock().expect("sessions").iter() {
            if !sessions.contains(session) {
                sessions.push(session.clone());
            }
        }
        sessions
    }
}

fn unavailable(error: impl std::fmt::Display) -> ApprovalBranchError {
    ApprovalBranchError::Unavailable(error.to_string())
}

fn work_target(session: &SessionId) -> NativeWorkTarget {
    NativeWorkTarget {
        logical_owner: agent("native-owner"),
        logical_runtime: id(&LogicalRuntimeId::for_session(session).to_string()),
        context: id("native-context"),
        context_generation: 1,
        audience: AudienceRef::Principal {
            principal: requester(),
        },
    }
}

/// The session's live controller client, pinned without queueing behind a
/// turn: a held turn boundary is tried again until it is free.
async fn pin(
    service: &meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>,
    session: &SessionId,
) -> ControllerModelClient {
    let actor = service
        .live_session_actor_witness(session)
        .await
        .expect("a live session");
    tokio::time::timeout(BOUND, async {
        loop {
            match service.try_pin_controller_client_for_actor(&actor).await {
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

#[async_trait]
impl ApprovalBranchHost for Branches {
    fn grantee_basis(&self, grantee: &PrincipalRef) -> Option<ApprovalGranteeBasis> {
        (grantee == &owner_agent()).then(|| self.basis.clone())
    }

    async fn open(&self, request: &ApprovalBranchRequest) -> Result<(), ApprovalBranchError> {
        let (_, session) = self
            .handle
            .open_approval_branch(&self.grantee, &request.decision)
            .await
            .map_err(unavailable)?;
        self.opened_sessions.lock().expect("sessions").push(session);
        Ok(())
    }

    async fn seat(&self, decision: &DecisionId) -> Result<ApprovalBranchSeat, ApprovalBranchError> {
        let branch = MobHandle::approval_branch_identity(decision);
        let session = self
            .handle
            .resolve_bridge_session_id(&branch)
            .await
            .ok_or_else(|| unavailable("the branch has no session yet"))?;
        let controller = pin(&self.service, &session).await;
        Ok(ApprovalBranchSeat::new(
            session.clone(),
            work_target(&session),
            controller,
        ))
    }

    fn enroll(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<Option<Box<dyn NativeInputAdmissionCustody>>, ApprovalBranchError> {
        if self.originals.enroll(association) {
            Ok(None)
        } else {
            Err(ApprovalBranchError::EnrollmentConflict)
        }
    }

    async fn admit(
        &self,
        decision: &DecisionId,
        input: Input,
        custody: Option<Box<dyn NativeInputAdmissionCustody>>,
    ) -> Result<ApprovedWorkAdmission, ApprovalBranchError> {
        let branch = MobHandle::approval_branch_identity(decision);
        let entry = self
            .handle
            .get_member(&branch)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| unavailable("the branch is not seated"))?;
        self.snapshots
            .lock()
            .expect("snapshots")
            .push(BranchSnapshot {
                identity: branch.clone(),
                labels: entry.labels.clone(),
                wired_to: entry.wired_to.iter().cloned().collect(),
            });
        let turn = self
            .handle
            .start_internal_governed_input_with_custody(
                &branch,
                input,
                custody.unwrap_or_else(|| Box::new(NoCustody)),
            )
            .await
            .map_err(|error| ApprovalBranchError::Refused(error.to_string()))?;
        let admission = turn
            .admission()
            .ok_or_else(|| unavailable("no canonical admission"))?;
        Ok(ApprovedWorkAdmission::new(
            admission.session_id,
            admission.input_id.to_string(),
            Box::pin(async move {
                let _ = turn.wait().await;
            }),
        ))
    }

    async fn retire(&self, decision: &DecisionId) -> Result<(), ApprovalBranchError> {
        self.handle
            .retire_approval_branch(decision)
            .await
            .map_err(unavailable)
    }

    async fn open_branches(&self) -> Result<Vec<DecisionId>, ApprovalBranchError> {
        Ok(self
            .handle
            .approval_branches()
            .await
            .into_iter()
            .map(|(decision, _)| decision)
            .collect())
    }
}

/// The host's requester reply route, recorded: one delivery per reply id,
/// unless `no_dedupe` (a route that delivers every call it is handed).
#[derive(Default)]
struct Route {
    deliveries: Mutex<HashMap<String, (String, ReplyReceipt)>>,
    delivered: Mutex<Vec<String>>,
    notices: Mutex<Vec<(DecisionId, AccessOutcome)>>,
    no_dedupe: AtomicBool,
}

#[async_trait]
impl RequesterReplyRoute for Route {
    fn can_deliver(&self, origin: &ApprovalOrigin) -> bool {
        origin.requester == requester()
    }

    async fn deliver(
        &self,
        _origin: &ApprovalOrigin,
        reply: &EvidenceId,
        text: &str,
    ) -> Result<ReplyReceipt, ReplyRouteError> {
        let mut deliveries = self.deliveries.lock().expect("deliveries");
        if !self.no_dedupe.load(Ordering::SeqCst)
            && let Some((delivered, receipt)) = deliveries.get(reply.as_str())
        {
            return if delivered == text {
                Ok(receipt.clone())
            } else {
                Err(ReplyRouteError::AlreadyDelivered)
            };
        }
        let receipt = ReplyReceipt {
            delivery: format!("delivery-{}", deliveries.len() + 1),
        };
        deliveries.insert(
            reply.as_str().to_owned(),
            (text.to_owned(), receipt.clone()),
        );
        self.delivered
            .lock()
            .expect("delivered")
            .push(text.to_owned());
        Ok(receipt)
    }

    async fn notify_origin(
        &self,
        _origin: &ApprovalOrigin,
        decision: &DecisionId,
        outcome: AccessOutcome,
    ) -> Option<ApprovalConsentDeliveryState> {
        self.notices
            .lock()
            .expect("notices")
            .push((decision.clone(), outcome));
        Some(ApprovalConsentDeliveryState::Failed {
            reason: format!("the origin was told: {}", outcome.as_str()),
        })
    }
}

/// Every session's controller transport. The owner's agent offers access on
/// its own question; the approved work runs the query twice (the second is
/// refused), replies three times (the same text twice, then another), then
/// answers.
#[derive(Default)]
struct ScriptedController {
    requests: Mutex<Vec<Vec<Message>>>,
}

fn mentions(messages: &[Message], needle: &str) -> bool {
    messages.iter().any(|message| {
        serde_json::to_string(message)
            .expect("message renders")
            .contains(needle)
    })
}

fn tool_results(messages: &[Message]) -> Vec<ToolResult> {
    messages
        .iter()
        .flat_map(|message| match message {
            Message::ToolResults { results, .. } => results.clone(),
            _ => Vec::new(),
        })
        .collect()
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

fn call(id: &'static str, name: &'static str, args: serde_json::Value) -> LlmStream<'static> {
    Box::pin(futures::stream::iter(vec![
        Ok(LlmEvent::ToolCallComplete {
            id: id.into(),
            name: name.into(),
            args,
            meta: None,
        }),
        Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::success(StopReason::ToolUse),
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
        let messages = request.request().messages.clone();
        self.requests
            .lock()
            .expect("requests")
            .push(messages.clone());
        if mentions(&messages, APPROVED_MARKER) {
            return match tool_results(&messages).len() {
                0 => call("query-1", SOURCE_TOOL, query_arguments()),
                1 => call("query-2", SOURCE_TOOL, query_arguments()),
                2 => call(
                    "reply-1",
                    "reply_to_requester",
                    serde_json::json!({"text": REPLY_TEXT}),
                ),
                3 => call(
                    "reply-2",
                    "reply_to_requester",
                    serde_json::json!({"text": REPLY_TEXT}),
                ),
                4 => call(
                    "reply-3",
                    "reply_to_requester",
                    serde_json::json!({"text": OTHER_REPLY}),
                ),
                _ => text("the approved work is done"),
            };
        }
        if mentions(&messages, SECOND_QUESTION) {
            return text("nothing more");
        }
        if mentions(&messages, LEDGER_PROMPT) {
            if tool_results(&messages)
                .iter()
                .any(|result| result.tool_use_id == LEDGER_OFFER_CALL)
            {
                return text(LEDGER_ANSWER);
            }
            return call(
                LEDGER_OFFER_CALL,
                "access_offer",
                serde_json::json!({
                    "operation": {
                        "server": "ledger",
                        "tool": TOOL,
                        "arguments": query_arguments(),
                    },
                    "summary": "the ledger's open invoices, for the requester",
                }),
            );
        }
        if mentions(&messages, OWNER_PROMPT) {
            if tool_results(&messages)
                .iter()
                .any(|result| result.tool_use_id == OFFER_CALL)
            {
                return text(OWNER_ANSWER);
            }
            return call(
                OFFER_CALL,
                "access_offer",
                serde_json::json!({
                    "operation": {
                        "server": SERVER,
                        "tool": TOOL,
                        "arguments": query_arguments(),
                    },
                    "summary": "last quarter's revenue by region, for the requester",
                }),
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
            peer_description: "governed owner's agent".to_string(),
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
        "approval-grants-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition.wiring = WiringRules {
        auto_wire_orchestrator: false,
        role_wiring: vec![],
    };
    definition
}

/// The grant owner's grants: the owner's agent's controller grant (which
/// may have one more level, its branches'), the owner's delegation for the
/// source under the owner's child rule (the offering grant), and its grant
/// to hold offers for the owner's decision. All act for the data owner.
struct Grants {
    owner: Arc<LocalGrantAuthority>,
    controller: GrantLineageRef,
    offering: GrantLineageRef,
    holds: GrantLineageRef,
}

impl Grants {
    fn new(decisions: &DurableDecisionService) -> Self {
        let owner = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: grant_root(),
                    namespace: id("approval-grants"),
                    generation: 1,
                },
                LocalAuthorizationPublication::new(),
                Arc::new(HostAuthorizationClock),
            )
            .expect("generated grant owner")
            .with_approval_use_currentness(Arc::new(
                DecisionApprovalUseCurrentness::new(decisions.clone(), Arc::new(Approvers)),
            )),
        );
        let mut delegable = ExecutionRestrictions::unrestricted();
        delegable.delegation_depth = DelegationDepth::remaining(1);
        let controller = owner
            .issue_root(
                &grant_root(),
                id("owner-agent-controller"),
                owner_agent(),
                Some(data_owner()),
                delegable.clone(),
            )
            .expect("the owner's agent's controller grant");
        let offering = owner
            .issue_root_with_child_rule(
                &grant_root(),
                id("owner-delegation"),
                owner_agent(),
                Some(data_owner()),
                delegable,
                ChildApprovalRule {
                    approver_id: approver(),
                    accepts_device_trust: GrantDeviceTrust::Refused,
                },
            )
            .expect("the owner's delegation, under its child rule");
        let holds = owner
            .issue_root(
                &grant_root(),
                id("owner-holds"),
                owner_agent(),
                Some(data_owner()),
                ExecutionRestrictions::unrestricted(),
            )
            .expect("the grant to hold offers");
        Self {
            owner,
            controller,
            offering,
            holds,
        }
    }
}

/// One persistent governed mob with the owner's agent seated, its
/// approval-carried grants composed, and the decision executor loop.
struct Flow {
    _realm: Arc<support::MemberRealm>,
    handle: MobHandle,
    machine: Arc<MeerkatMachine>,
    service: Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
    controller: Arc<ScriptedController>,
    originals: Arc<Originals>,
    grants: Grants,
    decisions: DurableDecisionService,
    branches: Arc<Branches>,
    route: Arc<Route>,
    source: Arc<Source>,
    executor: DecisionExecutorLoop,
    owner_member: AgentIdentity,
    owner_session: SessionId,
}

/// What one run of the flow left.
struct Outcome {
    flow: Flow,
    decision: StoredDecision,
}

impl Flow {
    async fn new() -> Self {
        Self::with_store(Arc::new(InMemoryDecisionStore::new())).await
    }

    /// The flow over the decision store `store`.
    async fn with_store(store: Arc<dyn DecisionStore>) -> Self {
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
        let decisions = DurableDecisionService::new(store);
        let grants = Grants::new(&decisions);
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
        let owner_member = AgentIdentity::from("owner-agent");
        let branches = Arc::new(Branches {
            handle: handle.clone(),
            service: service.clone(),
            originals: originals.clone(),
            grantee: owner_member.clone(),
            basis: ApprovalGranteeBasis::new(vec![grants.controller.clone()], Some(data_owner())),
            snapshots: Mutex::new(Vec::new()),
            opened_sessions: Mutex::new(Vec::new()),
        });
        let route = Arc::new(Route::default());
        let custody: Arc<dyn ApprovalControllerCustody> = machine.clone();
        let approval_grants = ApprovalGrants::new(ApprovalGrantsConfiguration::new(
            decisions.clone(),
            Arc::clone(&grants.owner),
            branches.clone(),
            route.clone(),
            Arc::downgrade(&custody),
            decision_ingress(),
        ))
        .expect("approval-carried grants");
        let source = Arc::new(Source::new());
        let tools: Arc<dyn AgentToolDispatcher> = Arc::new(
            meerkat_core::ToolGatewayBuilder::new()
                .add_dispatcher(approval_grants.tools())
                .add_dispatcher(source.clone())
                .build()
                .expect("the owner's agent's tools"),
        );
        let mut spec = SpawnMemberSpec::new("worker", owner_member.clone());
        spec.external_tools = Some(tools);
        handle
            .spawn_spec(spec)
            .await
            .expect("spawn the owner's agent");
        let owner_session = handle
            .resolve_bridge_session_id(&owner_member)
            .await
            .expect("the owner's agent's session");
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &machine.generated_auth_lease_handle(),
            selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-approval-grants-credential"),
            tokio::time::Instant::now() + BOUND,
        )
        .await
        .expect("fixture credential before native admission");
        let executor = DecisionExecutorLoop::start(
            decisions.clone(),
            [approval_grants.executor()],
            Arc::new(Approvers),
        )
        .expect("the decision executor loop");
        Self {
            _realm: realm,
            handle,
            machine,
            service,
            controller,
            originals,
            grants,
            decisions,
            branches,
            route,
            source,
            executor,
            owner_member,
            owner_session,
        }
    }

    /// The requester's own question `n` to the owner's agent: its basis
    /// selects the offering grant for the source's operations and the grant
    /// to hold offers for the owner's approver id, and no base.
    async fn ask_owner(&self, prompt: &str, n: usize) -> MemberTurnHandle {
        let pin = pin(&self.service, &self.owner_session).await;
        let association = InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: requester(),
            ingress_actor: host_ingress(),
            represented_subject: Some(data_owner()),
            original_authentication: evidence("requester-authentication"),
            logical_executor: owner_agent(),
            target: work_target(&self.owner_session),
            original_work: OriginalWorkRef {
                authority: host_ingress(),
                work: id(&format!("requester-question-{n}")),
            },
            root_event: evidence(&format!("requester-question-{n}-event")),
            contributing_work: Vec::new(),
            authority_basis: WorkAuthorityBasis::TargetedGrantLineages {
                base: None,
                targets: vec![
                    TargetGrantLineage {
                        target: GrantTarget::McpServer {
                            server: SERVER.into(),
                        },
                        lineage: vec![self.grants.offering.clone()],
                    },
                    TargetGrantLineage {
                        target: GrantTarget::DecisionHold {
                            approver_id: approver(),
                        },
                        lineage: vec![self.grants.holds.clone()],
                    },
                ],
            },
            controller_grant_lineage: vec![self.grants.controller.clone()],
            controller_model: Some(pin.selection().clone()),
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: Vec::new(),
            ingress_namespace: QualifiedIngressNamespace {
                realm: realm(),
                ingress: ingress_domain(),
                occurrence_scope: id(&format!("requester-question-{n}-occurrence")),
            },
            contract: ContractRequirements::local_governed_v1(),
        })
        .expect("the requester's question's claims");
        assert!(self.originals.enroll(&association));
        let event_key = association
            .candidate()
            .original_work
            .work
            .as_str()
            .to_owned();
        let input = bind_scoped_original(
            Input::Prompt(PromptInput::new(prompt, None)),
            association,
            IdempotencyKey::new(event_key),
            requester(),
            host_ingress(),
            realm(),
            evidence("transport-authentication"),
            pin,
        )
        .expect("the question is bound to its trusted ingress observation");
        self.handle
            .start_governed_input(&self.owner_member, input)
            .await
            .expect("the requester's question is admitted")
    }

    /// The one `grant.child` decision held.
    fn held(&self) -> StoredDecision {
        let listing = self
            .decisions
            .list(&DecisionListFilter::default(), None, 10)
            .expect("list decisions");
        let [decision] = listing.decisions.as_slice() else {
            panic!("one decision held: {:?}", listing.decisions);
        };
        decision.clone()
    }

    /// Present the held offer to the approver and approve it, as a person.
    fn approve(&self, decision: &StoredDecision) {
        let rendering = &decision.descriptor.rendering;
        let at_ms = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis(),
        )
        .expect("ms");
        self.decisions
            .present(
                &decision.decision_id,
                &PresentationRecord::complete(
                    PresentationAudience::Approver {
                        approver_id: approver(),
                    },
                    rendering,
                    None,
                    reference("chat"),
                    reference("message-1"),
                    at_ms,
                ),
            )
            .expect("present");
        let attested = AttestedDecision::new(
            AuthenticatedApprover::new(data_owner(), ApprovalAssurance::PersonAuthenticated)
                .expect("an authenticated person"),
            DecisionIngress::new("chat").expect("ingress"),
            reference("event-1"),
            at_ms + 1,
            approver(),
            rendering.subject_digest(),
            true,
        )
        .expect("attested decision");
        let decided = self
            .decisions
            .decide(&decision.decision_id, &attested, &Eligible)
            .expect("decide");
        assert_eq!(decided.state.phase, DecisionPhase::Approved);
    }

    /// The branch of `decision`, while it is a member.
    async fn branch_seated(&self, decision: &DecisionId) -> bool {
        self.handle
            .get_member(&MobHandle::approval_branch_identity(decision))
            .await
            .expect("roster read")
            .is_some()
    }

    /// Every model request of the approved work.
    fn approved_requests(&self) -> Vec<Vec<Message>> {
        self.controller
            .requests
            .lock()
            .expect("requests")
            .iter()
            .filter(|messages| mentions(messages, APPROVED_MARKER))
            .cloned()
            .collect()
    }

    /// Every model request of the owner's agent's own conversation.
    fn own_requests(&self) -> Vec<Vec<Message>> {
        self.controller
            .requests
            .lock()
            .expect("requests")
            .iter()
            .filter(|messages| {
                mentions(messages, OWNER_PROMPT) || mentions(messages, SECOND_QUESTION)
            })
            .cloned()
            .collect()
    }

    /// The approved work's result for the call `id`, from its last request.
    fn feedback(&self, id: &str) -> ToolResult {
        self.approved_requests()
            .last()
            .and_then(|messages| {
                tool_results(messages)
                    .into_iter()
                    .find(|result| result.tool_use_id == id)
            })
            .unwrap_or_else(|| panic!("a result for {id}"))
    }
}

async fn completed(turn: MemberTurnHandle, what: &str) {
    let outcome = tokio::time::timeout(BOUND, turn.wait())
        .await
        .unwrap_or_else(|_| panic!("{what} ends within bound"));
    assert!(outcome.is_ok(), "{what} completed: {outcome:?}");
}

/// The whole flow: the requester asks, the owner's agent offers, a person
/// approves, the executor executes and admits the approved work, which
/// runs in its branch and replies; the branch is then retired.
async fn run() -> Outcome {
    run_on(Flow::new().await).await
}

/// The whole flow on `flow`.
async fn run_on(mut flow: Flow) -> Outcome {
    completed(
        flow.ask_owner(OWNER_PROMPT, 1).await,
        "the requester's question",
    )
    .await;
    let decision = flow.held();
    assert_eq!(decision.state.phase, DecisionPhase::Open);
    flow.approve(&decision);
    let pass = flow.executor.pass().await;
    assert!(
        pass.outcomes
            .contains(&(decision.decision_id.clone(), DecisionPassOutcome::Executed)),
        "the approved offer executes: {pass:?}"
    );
    let pass = flow.executor.pass().await;
    assert!(
        pass.outcomes.contains(&(
            decision.decision_id.clone(),
            DecisionPassOutcome::NoticeSettled
        )),
        "its notice admits the approved work: {pass:?}"
    );
    // The approved work runs in its branch, replies, ends; the branch is
    // then retired.
    tokio::time::timeout(BOUND, async {
        while flow.branch_seated(&decision.decision_id).await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the branch is retired once its approved work ended");
    let decision = flow
        .decisions
        .get(&decision.decision_id)
        .expect("the decision");
    Outcome { flow, decision }
}

/// The access offer was held by the agent's own governed work: the
/// requester is the person who asked, the approver id is the offering
/// grant's rule's, and the model saw the decision it held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn access_offer_holds_the_offer_of_a_governed_work() {
    let Outcome { flow, decision } = run().await;
    assert_eq!(decision.state.phase, DecisionPhase::Executed);
    assert_eq!(
        decision.descriptor.requester,
        meerkat_core::approval::decision::DecisionPrincipalKey::of(&requester()).expect("key")
    );
    assert_eq!(decision.descriptor.approver_id, approver());
    let request = GrantChildRequest::of(&decision).expect("a grant.child request");
    assert_eq!(request.approval_branch_of, owner_agent());
    assert_eq!(request.terms.represented_subject, Some(data_owner()));
    let offered = flow
        .own_requests()
        .iter()
        .flat_map(|messages| tool_results(messages))
        .find(|result| result.tool_use_id == OFFER_CALL)
        .expect("the offer's result");
    assert!(!offered.is_error, "{}", offered.text_content());
    assert!(
        offered
            .text_content()
            .contains(decision.decision_id.as_str())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_approved_work_runs_in_a_one_time_branch_with_an_empty_transcript() {
    let Outcome { flow, decision } = run().await;
    let snapshots = flow.branches.snapshots.lock().expect("snapshots").clone();
    let [branch] = snapshots.as_slice() else {
        panic!("one branch ran the approved work: {snapshots:?}");
    };
    assert_eq!(
        branch.identity,
        MobHandle::approval_branch_identity(&decision.decision_id)
    );
    assert_ne!(branch.identity, flow.owner_member);
    let requests = flow.approved_requests();
    assert!(!requests.is_empty(), "the approved work reached the model");
    for messages in &requests {
        assert!(
            !mentions(messages, OWNER_PROMPT) && !mentions(messages, OWNER_ANSWER),
            "the branch holds nothing of the grantee's conversation: {messages:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_approved_result_never_reaches_the_grantees_own_session() {
    let Outcome { flow, decision } = run().await;
    // The grantee answers again in its own session, after the approved work.
    completed(
        flow.ask_owner(SECOND_QUESTION, 2).await,
        "the requester's second question",
    )
    .await;
    let own = flow.own_requests();
    assert!(
        own.iter()
            .any(|messages| mentions(messages, SECOND_QUESTION))
    );
    for messages in &own {
        assert!(
            !mentions(messages, SOURCE_RESULT) && !mentions(messages, REPLY_TEXT),
            "the grantee's own session holds nothing of the approved work: {messages:?}"
        );
    }
    // The origin was told only that access was approved.
    assert_eq!(
        *flow.route.notices.lock().expect("notices"),
        vec![(decision.decision_id.clone(), AccessOutcome::Approved)]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_console_viewer_and_the_distiller_see_nothing_of_the_approved_work() {
    // Core's half: the branch carries the authority label that makes a host
    // treat it as private to no person, and nothing converses with it.
    let Outcome { flow, decision } = run().await;
    let snapshots = flow.branches.snapshots.lock().expect("snapshots").clone();
    let [branch] = snapshots.as_slice() else {
        panic!("one branch: {snapshots:?}");
    };
    assert_eq!(
        branch.labels.get(APPROVAL_BRANCH_LABEL).map(String::as_str),
        Some(decision.decision_id.as_str())
    );
    assert!(branch.wired_to.is_empty(), "the branch is wired to nobody");
    assert!(
        flow.handle
            .approval_branches()
            .await
            .iter()
            .all(|(open, _)| open != &decision.decision_id),
        "and it is retired"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_branch_is_retired_when_the_reply_settles() {
    let Outcome { flow, decision } = run().await;
    assert!(!flow.branch_seated(&decision.decision_id).await);
    assert_eq!(
        *flow.route.delivered.lock().expect("delivered"),
        vec![REPLY_TEXT.to_owned()]
    );
    // Its controller grant is revoked with it.
    let branch = approval_branch_principal(&owner_agent(), &decision.decision_id).expect("branch");
    let controller = flow
        .grants
        .owner
        .issued_reference(&branch_controller_grant_id(&decision.decision_id).expect("id"))
        .expect("owner")
        .expect("the branch's controller grant was issued");
    let lineage = vec![flow.grants.controller.clone(), controller];
    tokio::time::timeout(BOUND, async {
        while flow
            .grants
            .owner
            .resolve_controller_lineage(&lineage, &branch, Some(&data_owner()))
            .is_ok()
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the branch's controller grant is revoked once the branch retired");
    let _ = &flow.machine;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_approved_work_is_admitted_by_the_installed_ingress_check() {
    let Outcome { flow, decision } = run().await;
    let work = approved_work_id(&decision.decision_id).expect("approved work id");
    let enrolled = flow
        .originals
        .enrolled_work(&work)
        .expect("the approved work was enrolled before its admission");
    let admitted = flow.originals.admitted.lock().expect("admitted").clone();
    assert!(
        admitted.contains(&enrolled),
        "native admission authenticated the approved work against the host's custody"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_approved_work_has_one_target_lineage_and_no_base() {
    let Outcome { flow, decision } = run().await;
    let work = approved_work_id(&decision.decision_id).expect("approved work id");
    let enrolled = flow.originals.enrolled_work(&work).expect("enrolled");
    let approval = flow
        .grants
        .owner
        .issued_reference(&approval_grant_id(&decision.decision_id).expect("id"))
        .expect("owner")
        .expect("the approval-carried grant");
    assert_eq!(
        enrolled.candidate().authority_basis,
        WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: vec![TargetGrantLineage {
                target: GrantTarget::McpServer {
                    server: SERVER.into()
                },
                lineage: vec![flow.grants.offering, approval],
            }],
        }
    );
    assert!(enrolled.candidate().contributing_work.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_approved_work_keeps_the_requester_and_takes_the_grants_subject() {
    let Outcome { flow, decision } = run().await;
    let work = approved_work_id(&decision.decision_id).expect("approved work id");
    let enrolled = flow.originals.enrolled_work(&work).expect("enrolled");
    let claims = enrolled.candidate();
    assert_eq!(claims.requester, requester());
    assert_eq!(claims.represented_subject, Some(data_owner()));
    assert_eq!(claims.ingress_actor, decision_ingress());
    assert_eq!(
        Some(claims.logical_executor.clone()),
        approval_branch_principal(&owner_agent(), &decision.decision_id)
    );
    assert_eq!(
        claims.original_work,
        OriginalWorkRef {
            authority: host_ingress(),
            work,
        }
    );
    assert_eq!(claims.ingress_namespace.realm, realm());
    assert_eq!(claims.ingress_namespace.ingress, ingress_domain());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_approved_work_queries_once_and_replies_once() {
    let Outcome { flow, .. } = run().await;
    assert_eq!(
        flow.source.runs.load(Ordering::SeqCst),
        1,
        "the source ran exactly once"
    );
    let first = flow.feedback("query-1");
    assert!(!first.is_error, "{}", first.text_content());
    assert!(first.text_content().contains(SOURCE_RESULT));
    let second = flow.feedback("query-2");
    assert!(
        second.is_error && second.text_content().contains("operation_refused"),
        "the one use is spent: {}",
        second.text_content()
    );
    let replied = flow.feedback("reply-1");
    assert!(!replied.is_error, "{}", replied.text_content());
    let again = flow.feedback("reply-2");
    assert!(!again.is_error, "{}", again.text_content());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&replied.text_content()).expect("json")["delivery"],
        serde_json::from_str::<serde_json::Value>(&again.text_content()).expect("json")["delivery"],
        "the same reply answers the first delivery's receipt"
    );
    let other = flow.feedback("reply-3");
    assert!(
        other.is_error && other.text_content().contains("reply_already_settled"),
        "{}",
        other.text_content()
    );
    assert_eq!(
        *flow.route.delivered.lock().expect("delivered"),
        vec![REPLY_TEXT.to_owned()],
        "one reply reached the requester"
    );
}

/// The reply of an approved work is once, by its decision: a route that
/// delivers every call it is handed never sees a second, other text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reply_once_holds_when_the_hosts_route_delivers_every_call() {
    let flow = Flow::new().await;
    flow.route.no_dedupe.store(true, Ordering::SeqCst);
    let Outcome { flow, .. } = run_on(flow).await;
    let other = flow.feedback("reply-3");
    assert!(
        other.is_error && other.text_content().contains("reply_already_settled"),
        "{}",
        other.text_content()
    );
    let delivered = flow.route.delivered.lock().expect("delivered").clone();
    assert!(
        !delivered.iter().any(|text| text == OTHER_REPLY),
        "another text never reaches the requester: {delivered:?}"
    );
    assert!(delivered.iter().all(|text| text == REPLY_TEXT));
}

/// The decision store, whose next settlement of an owed notice fails before
/// it is written, as a store conflict or a crash would leave it.
struct SettleFails {
    inner: InMemoryDecisionStore,
    failures: AtomicUsize,
}

impl DecisionStore for SettleFails {
    fn get(
        &self,
        decision_id: &DecisionId,
    ) -> Result<
        Option<meerkat_core::approval::decision::DecisionRow>,
        meerkat_core::approval::ApprovalStoreError,
    > {
        self.inner.get(decision_id)
    }

    fn insert_if_absent(
        &self,
        row: &StoredDecision,
    ) -> Result<(), meerkat_core::approval::ApprovalStoreError> {
        self.inner.insert_if_absent(row)
    }

    fn replace_if_revision(
        &self,
        expected_revision: u64,
        row: &StoredDecision,
    ) -> Result<(), meerkat_core::approval::ApprovalStoreError> {
        let settles = row.notice_receipt.is_some()
            && matches!(
                self.inner.get(&row.decision_id),
                Ok(Some(meerkat_core::approval::decision::DecisionRow::Readable(stored)))
                    if stored.notice_receipt.is_none()
            );
        if settles
            && self
                .failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok()
        {
            return Err(meerkat_core::approval::ApprovalStoreError::NotCommitted(
                "the settlement was lost".into(),
            ));
        }
        self.inner.replace_if_revision(expected_revision, row)
    }

    fn list(
        &self,
        filter: &DecisionListFilter,
        after: Option<&str>,
        limit: usize,
    ) -> Result<
        meerkat_core::approval::decision::DecisionPage,
        meerkat_core::approval::ApprovalStoreError,
    > {
        self.inner.list(filter, after, limit)
    }

    fn owed(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<
        meerkat_core::approval::decision::DecisionPage,
        meerkat_core::approval::ApprovalStoreError,
    > {
        self.inner.owed(after, limit)
    }

    fn due(
        &self,
        now_ms: u64,
        after: Option<&str>,
        limit: usize,
    ) -> Result<
        meerkat_core::approval::decision::DecisionPage,
        meerkat_core::approval::ApprovalStoreError,
    > {
        self.inner.due(now_ms, after, limit)
    }

    fn count(
        &self,
        filter: &DecisionListFilter,
    ) -> Result<u64, meerkat_core::approval::ApprovalStoreError> {
        self.inner.count(filter)
    }

    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }

    fn claim_executor_kind(
        &self,
        kind: &meerkat_core::approval::decision::DecisionKind,
    ) -> Result<
        meerkat_core::approval::decision::DecisionExecutorClaim,
        meerkat_core::approval::ApprovalStoreError,
    > {
        self.inner.claim_executor_kind(kind)
    }
}

/// The Executed notice delivered again after its work ran and its branch
/// retired forks nothing: the branch the execution opened was the only one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_redelivered_executed_notice_forks_nothing_and_settles() {
    let mut flow = Flow::with_store(Arc::new(SettleFails {
        inner: InMemoryDecisionStore::new(),
        failures: AtomicUsize::new(1),
    }))
    .await;
    completed(
        flow.ask_owner(OWNER_PROMPT, 1).await,
        "the requester's question",
    )
    .await;
    let decision = flow.held();
    flow.approve(&decision);
    let pass = flow.executor.pass().await;
    assert!(
        pass.outcomes
            .contains(&(decision.decision_id.clone(), DecisionPassOutcome::Executed)),
        "{pass:?}"
    );
    // The notice admits the approved work, and its settlement is lost.
    let pass = flow.executor.pass().await;
    assert!(
        pass.outcomes
            .iter()
            .any(|(id, outcome)| id == &decision.decision_id
                && matches!(outcome, DecisionPassOutcome::Failed(_))),
        "{pass:?}"
    );
    // The approved work runs, replies and ends: its branch retires.
    tokio::time::timeout(BOUND, async {
        while flow.branch_seated(&decision.decision_id).await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the branch is retired once its approved work ended");
    // The notice is delivered again: it forks no new branch, and settles.
    let pass = flow.executor.pass().await;
    assert!(
        pass.outcomes.contains(&(
            decision.decision_id.clone(),
            DecisionPassOutcome::NoticeSettled
        )),
        "{pass:?}"
    );
    assert_eq!(
        flow.branches.branch_sessions().len(),
        1,
        "the redelivered notice forked no new branch"
    );
    assert_eq!(
        flow.branches.snapshots.lock().expect("snapshots").len(),
        1,
        "the approved work was admitted once"
    );
    assert!(!flow.branch_seated(&decision.decision_id).await);
    // The redelivery never settles silently: it tells the origin how its
    // request ended, from the decision's reply fact (the work settled its
    // reply, so approved), and settles with the origin's receipt.
    assert_eq!(
        *flow.route.notices.lock().expect("notices"),
        vec![
            (decision.decision_id.clone(), AccessOutcome::Approved),
            (decision.decision_id.clone(), AccessOutcome::Approved),
        ],
        "the redelivered notice told the origin again"
    );
    assert_eq!(
        flow.decisions
            .get(&decision.decision_id)
            .expect("the decision")
            .notice_receipt,
        Some(ApprovalConsentDeliveryState::Failed {
            reason: "the origin was told: approved".into()
        })
    );
}

/// The refusal a reserved approval branch path answers: typed, naming the
/// member it refused, with its token.
fn assert_reserved(error: &meerkat_mob::MobError, identity: &AgentIdentity, path: &str) {
    assert!(
        matches!(
            error,
            meerkat_mob::MobError::ApprovalBranchReserved { identity: refused }
                if refused == identity
        ),
        "{path}: {error:?}"
    );
    assert!(
        error.to_string().contains("approval_branch_reserved"),
        "{path}: {error}"
    );
}

/// The id of a `grant.child` decision of the subject `subject`.
fn a_decision(subject: &str) -> DecisionId {
    DecisionId::derive(
        &meerkat_core::approval::decision::DecisionKind::new("grant.child").expect("kind"),
        &meerkat_core::approval::decision::DecisionSubjectKey::new(subject).expect("subject key"),
    )
}

/// A member planted as the branch of a decision, by its identity or its
/// label, is refused on every spawn path but `open_approval_branch`, which
/// opens the branch itself and records the decision as its creation source.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_planted_with_the_branch_identity_or_label_is_refused() {
    let flow = Flow::new().await;
    let decision = a_decision("planted");
    let identity = MobHandle::approval_branch_identity(&decision);
    let label = BTreeMap::from([(
        APPROVAL_BRANCH_LABEL.to_owned(),
        decision.as_str().to_owned(),
    )]);
    let planted = SpawnMemberSpec::new("worker", identity.clone()).with_labels(label.clone());
    assert_reserved(
        &flow
            .handle
            .spawn_spec(planted.clone())
            .await
            .expect_err("a member planted with the branch identity and label is refused"),
        &identity,
        "spawn",
    );
    assert_reserved(
        &flow
            .handle
            .spawn_spec(SpawnMemberSpec::new("worker", identity.clone()))
            .await
            .expect_err("the branch identity prefix is reserved"),
        &identity,
        "spawn (identity)",
    );
    let ordinary = AgentIdentity::from("ordinary-member");
    assert_reserved(
        &flow
            .handle
            .spawn_spec(SpawnMemberSpec::new("worker", ordinary.clone()).with_labels(label.clone()))
            .await
            .expect_err("the branch label is reserved"),
        &ordinary,
        "spawn (label)",
    );
    assert_reserved(
        &flow
            .handle
            .ensure_member(planted.clone())
            .await
            .expect_err("ensure refuses the branch identity"),
        &identity,
        "ensure",
    );
    assert_reserved(
        &flow
            .handle
            .reconcile(
                vec![planted.clone()],
                meerkat_mob::runtime::ReconcileOptions::default(),
            )
            .await
            .expect_err("a reconcile refuses the branch identity"),
        &identity,
        "reconcile",
    );
    assert_reserved(
        &flow
            .handle
            .fork_member(&flow.owner_member, planted, None)
            .await
            .expect_err("a fork refuses the branch identity"),
        &identity,
        "fork",
    );
    assert!(
        flow.handle
            .get_member(&identity)
            .await
            .expect("roster read")
            .is_none(),
        "nothing was seated under the branch identity"
    );
    assert!(
        flow.handle
            .get_member(&ordinary)
            .await
            .expect("roster read")
            .is_none(),
        "nor under the label"
    );
    // `open_approval_branch` opens the branch itself: a fork of the grantee
    // that records the decision as its creation source.
    let (member, session) = flow
        .handle
        .open_approval_branch(&flow.owner_member, &decision)
        .await
        .expect("the branch opens");
    assert_eq!(member, identity);
    assert_ne!(session, flow.owner_session);
    let entry = flow
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the branch is seated");
    assert_eq!(entry.approval_branch(), Some(&decision));
    assert_eq!(
        flow.handle.approval_branches().await,
        vec![(decision.clone(), identity.clone())]
    );
    // Opening it again adopts the branch its own path opened.
    assert_eq!(
        flow.handle
            .open_approval_branch(&flow.owner_member, &decision)
            .await
            .expect("the branch is adopted"),
        (identity, session)
    );
}

/// An open approval branch is wired to nobody and cannot be wired, unwired
/// or respawned through the mob's controls.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_open_approval_branch_cannot_be_wired_unwired_or_respawned() {
    let flow = Flow::new().await;
    let decision = a_decision("opened");
    let (branch, _) = flow
        .handle
        .open_approval_branch(&flow.owner_member, &decision)
        .await
        .expect("the branch opens");
    assert_reserved(
        &flow
            .handle
            .wire(flow.owner_member.clone(), branch.clone())
            .await
            .expect_err("nothing is wired to an approval branch"),
        &branch,
        "wire",
    );
    assert_reserved(
        &flow
            .handle
            .wire(branch.clone(), flow.owner_member.clone())
            .await
            .expect_err("an approval branch is wired to nothing"),
        &branch,
        "wire (from the branch)",
    );
    assert_reserved(
        &flow
            .handle
            .wire_members_batch([(flow.owner_member.clone(), branch.clone())])
            .await
            .expect_err("nor in a batch"),
        &branch,
        "wire batch",
    );
    assert_reserved(
        &flow
            .handle
            .unwire(flow.owner_member.clone(), branch.clone())
            .await
            .expect_err("an approval branch's wiring is not the mob's to change"),
        &branch,
        "unwire",
    );
    match flow
        .handle
        .respawn(branch.clone(), None)
        .await
        .expect_err("an approval branch is never re-seeded")
    {
        meerkat_mob::MobRespawnError::Mob(error) => assert_reserved(&error, &branch, "respawn"),
        other => panic!("respawn: expected the reservation's refusal, got {other:?}"),
    }
    // Nor is it ensured: an ensure of its identity answers nothing of it.
    assert_reserved(
        &flow
            .handle
            .ensure_member(SpawnMemberSpec::new("worker", branch.clone()))
            .await
            .expect_err("an open branch is never ensured"),
        &branch,
        "ensure (open)",
    );
    let entry = flow
        .handle
        .get_member(&branch)
        .await
        .expect("roster read")
        .expect("the branch stays open");
    assert!(entry.wired_to.is_empty(), "{:?}", entry.wired_to);
    assert_eq!(entry.approval_branch(), Some(&decision));
}

/// The JSON payload of the result of the call `id` in `messages`.
fn payload_of(messages: &[Message], id: &str) -> serde_json::Value {
    let result = tool_results(messages)
        .into_iter()
        .find(|result| result.tool_use_id == id)
        .unwrap_or_else(|| panic!("a result for {id}"));
    assert!(result.is_error, "{}", result.text_content());
    serde_json::from_str(&result.text_content()).expect("a JSON error payload")
}

fn assert_no_parent_grant(payload: &serde_json::Value) {
    assert_eq!(payload["error"], "execution_failed", "{payload}");
    assert_eq!(
        payload["data"],
        serde_json::json!({"refusal": "no_child_approval_rule", "reason": "no_parent_grant"}),
        "{payload}"
    );
}

/// An offer of a source no grant of the work covers is refused when its
/// plan resolves, and the model sees the typed refusal with its data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_model_sees_a_plan_time_offer_refusal_with_its_data() {
    let flow = Flow::new().await;
    completed(
        flow.ask_owner(LEDGER_PROMPT, 1).await,
        "the requester's ledger question",
    )
    .await;
    let requests = flow.controller.requests.lock().expect("requests").clone();
    let answered = requests
        .iter()
        .find(|messages| {
            tool_results(messages)
                .iter()
                .any(|result| result.tool_use_id == LEDGER_OFFER_CALL)
        })
        .expect("the model saw the offer's result");
    assert_no_parent_grant(&payload_of(answered, LEDGER_OFFER_CALL));
    assert!(
        flow.decisions
            .list(&DecisionListFilter::default(), None, 10)
            .expect("list")
            .decisions
            .is_empty(),
        "nothing was held"
    );
}

/// The same refusal as a client reads it: the session's committed
/// transcript (`session/history`) carries the typed refusal with its data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_reads_a_plan_time_offer_refusal_with_its_data() {
    use meerkat_core::SessionServiceHistoryExt as _;
    use meerkat_core::service::SessionHistoryQuery;
    let flow = Flow::new().await;
    completed(
        flow.ask_owner(LEDGER_PROMPT, 1).await,
        "the requester's ledger question",
    )
    .await;
    let history = flow
        .service
        .read_history(&flow.owner_session, SessionHistoryQuery::default())
        .await
        .expect("the session's history");
    assert_no_parent_grant(&payload_of(&history.messages, LEDGER_OFFER_CALL));
}
