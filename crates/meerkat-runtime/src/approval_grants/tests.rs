//! Rows of approval-carried governed child grants: holding an access offer,
//! executing it, and retiring its branch, over the real generated grant and
//! decision owners and the host seams' fakes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_authorization::clock::{
    LocalAuthorizationClock, LocalAuthorizationTime, LocalClockError,
};
use meerkat_authorization::grants::{
    ChildApprovalRule, ChildGrantOutcome, ChildGrantValidity, GrantDeviceTrust, GrantOwnerStore,
    LocalGrantAuthority, LocalGrantConfiguration,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::constraints::{
    AudienceRef, DelegationDepth, ExactRestriction, ExecutionRestrictions, LifetimeRestriction,
    ProcessorRef, ResourceDomain,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::grant_mutation::{
    ControllerCustodyRefusal, ControllerGrantMutationCustody,
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
    ApproverId, AttestedDecision, DecisionId, DecisionIngress, DecisionPhase, DecisionReference,
    DecisionStore, InMemoryDecisionStore, PresentationAudience, PresentationRecord, StoredDecision,
};
use meerkat_core::approval::{ApprovalAssurance, ApprovalConsentDeliveryState};
use meerkat_core::connection::RealmId;
use meerkat_core::time_compat::Instant;
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

use super::offer::{
    AccessOfferArguments, AccessOfferError, CallingWork, OperationArguments,
    branch_controller_intent,
};
use super::*;
use crate::decision::{
    DecisionApproverCurrentness, DecisionClockError, DecisionExecutor, DecisionExecutorLoop,
    DecisionPassOutcome,
};
use crate::input_authority::GovernedPeerOrigin;

const DOMAIN: &str = "approval-grants";
const SERVER: &str = "records";
const TOOL: &str = "query";
const APPROVER: &str = "data-owner";
const MINUTE_MS: u64 = 60 * 1000;

fn principal(kind: PrincipalKind, value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(kind, value, TrustDomainId::new(DOMAIN).expect("domain"))
        .expect("qualified principal")
}

fn agent(value: &str) -> PrincipalRef {
    principal(PrincipalKind::ServiceAccount, value)
}

fn requester() -> PrincipalRef {
    principal(PrincipalKind::Human, "requester")
}

fn data_agent() -> PrincipalRef {
    agent("data-agent")
}

fn owner_agent() -> PrincipalRef {
    agent("owner-agent")
}

fn host_ingress() -> PrincipalRef {
    agent("host-ingress")
}

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded identifier")
}

fn approver() -> ApproverId {
    ApproverId::new(APPROVER).expect("approver id")
}

fn reference(value: &str) -> DecisionReference {
    DecisionReference::new(value).expect("reference")
}

fn real_now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after the epoch")
            .as_millis(),
    )
    .expect("milliseconds fit")
}

/// The host's wall clock, moved forward by an offset the rows set: both the
/// grant owner and the decision owner read it.
#[derive(Default)]
struct TestClock {
    offset_ms: AtomicU64,
}

impl TestClock {
    fn advance(&self, ms: u64) {
        self.offset_ms.fetch_add(ms, Ordering::SeqCst);
    }

    fn now(&self) -> u64 {
        real_now_ms() + self.offset_ms.load(Ordering::SeqCst)
    }
}

impl LocalAuthorizationClock for TestClock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        Ok(LocalAuthorizationTime {
            unix_ms: TestClock::now(self),
            monotonic: Instant::now()
                + std::time::Duration::from_millis(self.offset_ms.load(Ordering::SeqCst)),
        })
    }
}

impl crate::decision::DecisionClock for TestClock {
    fn now_ms(&self) -> Result<u64, DecisionClockError> {
        Ok(TestClock::now(self))
    }
}

/// A grant custody that vetoes nothing.
struct AnyCustody;

impl ControllerGrantMutationCustody for AnyCustody {
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _reference: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
        Ok(mutate())
    }
}

/// The native custody of a runtime whose branches run no unfinished work.
struct Custody;

impl ApprovalControllerCustody for Custody {
    fn revoke_controller(
        &self,
        grants: &LocalGrantAuthority,
        issuer: &PrincipalRef,
        grant: &GrantLineageRef,
    ) -> Result<(), meerkat_authorization::grants::GrantRefusal> {
        grants.revoke(issuer, grant, &mut AnyCustody)
    }
}

/// The host's approval branches, recorded: which grantees it can branch
/// (and on which controller lineage), which branches were opened and
/// retired, and every call Core made. It seats nothing (or refuses the seat
/// for good, `seat_refused`): these rows stop before an approved work runs.
#[derive(Default)]
struct Branches {
    bases: Mutex<HashMap<String, ApprovalGranteeBasis>>,
    opened: Mutex<Vec<ApprovalBranchRequest>>,
    retired: Mutex<Vec<DecisionId>>,
    calls: Mutex<Vec<&'static str>>,
    seat_refused: AtomicBool,
}

impl Branches {
    fn can_branch(&self, grantee: &PrincipalRef, basis: ApprovalGranteeBasis) {
        self.bases
            .lock()
            .expect("bases")
            .insert(grantee.id.as_str().to_owned(), basis);
    }

    fn opened(&self) -> Vec<ApprovalBranchRequest> {
        self.opened.lock().expect("opened").clone()
    }

    fn retired(&self) -> Vec<DecisionId> {
        self.retired.lock().expect("retired").clone()
    }

    fn call(&self, call: &'static str) {
        self.calls.lock().expect("calls").push(call);
    }

    /// Every call Core made of the host's branches, in order.
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().expect("calls").clone()
    }
}

#[async_trait]
impl ApprovalBranchHost for Branches {
    fn grantee_basis(&self, grantee: &PrincipalRef) -> Option<ApprovalGranteeBasis> {
        self.call("grantee_basis");
        self.bases
            .lock()
            .expect("bases")
            .get(grantee.id.as_str())
            .cloned()
    }

    async fn open(&self, request: &ApprovalBranchRequest) -> Result<(), ApprovalBranchError> {
        self.call("open");
        self.opened.lock().expect("opened").push(request.clone());
        Ok(())
    }

    async fn seat(
        &self,
        _decision: &DecisionId,
    ) -> Result<ApprovalBranchSeat, ApprovalBranchError> {
        self.call("seat");
        if self.seat_refused.load(Ordering::SeqCst) {
            return Err(ApprovalBranchError::Refused(
                "the branch cannot run the approved work".into(),
            ));
        }
        Err(ApprovalBranchError::Unavailable(
            "no seat in these rows".into(),
        ))
    }

    fn enroll(
        &self,
        _association: &InputAuthorityAssociation,
    ) -> Result<
        Option<Box<dyn crate::input_admission_custody::NativeInputAdmissionCustody>>,
        ApprovalBranchError,
    > {
        self.call("enroll");
        Err(ApprovalBranchError::Unavailable(
            "no enrollment in these rows".into(),
        ))
    }

    async fn admit(
        &self,
        _decision: &DecisionId,
        _input: crate::input::Input,
        _custody: Option<Box<dyn crate::input_admission_custody::NativeInputAdmissionCustody>>,
    ) -> Result<ApprovedWorkAdmission, ApprovalBranchError> {
        self.call("admit");
        Err(ApprovalBranchError::Unavailable(
            "no admission in these rows".into(),
        ))
    }

    async fn retire(&self, decision: &DecisionId) -> Result<(), ApprovalBranchError> {
        self.call("retire");
        self.retired.lock().expect("retired").push(decision.clone());
        Ok(())
    }

    async fn open_branches(&self) -> Result<Vec<DecisionId>, ApprovalBranchError> {
        self.call("open_branches");
        let retired = self.retired();
        Ok(self
            .opened()
            .into_iter()
            .map(|request| request.decision)
            .filter(|decision| !retired.contains(decision))
            .collect())
    }
}

/// The host's requester reply route, recorded.
struct Route {
    deliverable: AtomicBool,
    notices: Mutex<Vec<(DecisionId, AccessOutcome)>>,
}

impl Default for Route {
    fn default() -> Self {
        Self {
            deliverable: AtomicBool::new(true),
            notices: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl RequesterReplyRoute for Route {
    fn can_deliver(&self, _origin: &ApprovalOrigin) -> bool {
        self.deliverable.load(Ordering::SeqCst)
    }

    async fn deliver(
        &self,
        _origin: &ApprovalOrigin,
        _reply: &EvidenceId,
        _text: &str,
    ) -> Result<ReplyReceipt, ReplyRouteError> {
        Err(ReplyRouteError::Unavailable(
            "no replies in these rows".into(),
        ))
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
            reason: format!("origin told: {}", outcome.as_str()),
        })
    }
}

struct Eligible;

impl ConsentEligibility for Eligible {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}

/// The host's approvers: every decider is current.
struct Approvers;

impl DecisionApproverCurrentness for Approvers {
    fn approver_answer(&self, _decision: &StoredDecision) -> ConsentEligibilityAnswer {
        ConsentEligibilityAnswer::Eligible(Arc::new(Eligible))
    }
}

/// One host's approval-carried grants: the generated grant and decision
/// owners on one clock, the offering grant under the owner's child rule,
/// the asking agent's controller grant, and the host seams' fakes.
struct Fixture {
    clock: Arc<TestClock>,
    owner: Arc<LocalGrantAuthority>,
    decisions: crate::decision::DurableDecisionService,
    branches: Arc<Branches>,
    route: Arc<Route>,
    _custody: Arc<dyn ApprovalControllerCustody>,
    grants: ApprovalGrants,
    /// The owner's delegation to its agent: the offering grant.
    offering: Vec<GrantLineageRef>,
    /// The asking agent's controller grant.
    controller: Vec<GrantLineageRef>,
    /// The grant owner's journal, when the owner is durable.
    grant_store: Option<Arc<dyn GrantOwnerStore>>,
}

/// How the fixture's grants are issued, and which owners and seams it
/// composes.
struct Terms {
    /// The offering grant's subject.
    offering_subject: Option<PrincipalRef>,
    /// The offering grant's child rule.
    rule: bool,
    /// The offering grant's lifetime.
    offering_lifetime: Option<LifetimeRestriction>,
    /// The offering grant's processors and audiences bounds.
    offering_processors: Option<ExactRestriction<ProcessorRef>>,
    offering_audiences: Option<ExactRestriction<AudienceRef>>,
    /// The asking agent's controller grant's subject.
    controller_subject: Option<PrincipalRef>,
    /// The asking agent's controller grant's remaining depth.
    controller_depth: u32,
    /// The native custody that revokes a branch's controller grant.
    custody: Option<Arc<dyn ApprovalControllerCustody>>,
    /// The decision store (the in-memory store by default).
    store: Option<Arc<dyn DecisionStore>>,
    /// The grant owner's journal: a durable owner over it, when set.
    grant_store: Option<Arc<dyn GrantOwnerStore>>,
    /// The host's approvers, composed into the grant owner's use-time
    /// currentness ([`DecisionApprovalUseCurrentness`]), when set.
    use_approvers: Option<Arc<dyn DecisionApproverCurrentness>>,
}

impl Default for Terms {
    fn default() -> Self {
        Self {
            offering_subject: None,
            rule: true,
            offering_lifetime: None,
            offering_processors: None,
            offering_audiences: None,
            controller_subject: None,
            controller_depth: 1,
            custody: None,
            store: None,
            grant_store: None,
            use_approvers: None,
        }
    }
}

fn grant_configuration() -> LocalGrantConfiguration {
    LocalGrantConfiguration {
        root: agent("grant-root"),
        namespace: id("approval-grants"),
        generation: 1,
    }
}

/// The grant owner's journal, whose appends end uncertain while `uncertain`:
/// nothing is written, and the owner becomes unavailable until it restarts.
#[derive(Default)]
struct UncertainJournal {
    inner: meerkat_authorization::grants::InMemoryGrantOwnerStore,
    uncertain: AtomicBool,
}

impl GrantOwnerStore for UncertainJournal {
    fn load(
        &self,
    ) -> Result<
        Option<meerkat_authorization::grants::DurableGrantOwner>,
        meerkat_authorization::grants::GrantStoreError,
    > {
        self.inner.load()
    }

    fn create(
        &self,
        identity: &meerkat_authorization::grants::GrantOwnerIdentity,
    ) -> Result<(), meerkat_authorization::grants::GrantStoreError> {
        self.inner.create(identity)
    }

    fn append(
        &self,
        change: &meerkat_authorization::grants::GrantOwnerChange,
    ) -> Result<(), meerkat_authorization::grants::GrantStoreError> {
        if self.uncertain.load(Ordering::SeqCst) {
            return Err(meerkat_authorization::grants::GrantStoreError::Uncertain);
        }
        self.inner.append(change)
    }

    fn release(&self) {
        self.inner.release();
    }
}

/// The host's composition of approval-carried grants over these owners and
/// seams.
fn compose(
    decisions: &crate::decision::DurableDecisionService,
    owner: &Arc<LocalGrantAuthority>,
    branches: &Arc<Branches>,
    route: &Arc<Route>,
    custody: &Arc<dyn ApprovalControllerCustody>,
    clock: &Arc<TestClock>,
) -> ApprovalGrants {
    ApprovalGrants::new(
        ApprovalGrantsConfiguration::new(
            decisions.clone(),
            Arc::clone(owner),
            branches.clone(),
            route.clone(),
            Arc::downgrade(custody),
            agent("decision-ingress"),
        )
        .with_clock(clock.clone()),
    )
    .expect("approval-carried grants")
}

impl Fixture {
    fn new() -> Self {
        Self::with(Terms::default())
    }

    fn with(terms: Terms) -> Self {
        let clock = Arc::new(TestClock::default());
        let store: Arc<dyn DecisionStore> = terms
            .store
            .unwrap_or_else(|| Arc::new(InMemoryDecisionStore::new()));
        let decisions = crate::decision::DurableDecisionService::with_clock(store, clock.clone());
        let owner = match &terms.grant_store {
            Some(grant_store) => LocalGrantAuthority::new_durable(
                grant_configuration(),
                Arc::clone(grant_store),
                LocalAuthorizationPublication::new(),
                clock.clone(),
            )
            .expect("durable grant owner"),
            None => LocalGrantAuthority::new(
                grant_configuration(),
                LocalAuthorizationPublication::new(),
                clock.clone(),
            )
            .expect("grant owner"),
        };
        let owner = Arc::new(match terms.use_approvers {
            Some(approvers) => owner.with_approval_use_currentness(Arc::new(
                DecisionApprovalUseCurrentness::new(decisions.clone(), approvers),
            )),
            None => owner,
        });
        let mut offering_bounds = ExecutionRestrictions::unrestricted();
        offering_bounds.delegation_depth = DelegationDepth::remaining(1);
        if let Some(lifetime) = terms.offering_lifetime {
            offering_bounds.lifetime = lifetime;
        }
        if let Some(processors) = terms.offering_processors {
            offering_bounds.processors = processors;
        }
        if let Some(audiences) = terms.offering_audiences {
            offering_bounds.audiences = audiences;
        }
        let offering = if terms.rule {
            owner.issue_root_with_child_rule(
                &agent("grant-root"),
                id("owner-delegation"),
                owner_agent(),
                terms.offering_subject.clone(),
                offering_bounds,
                ChildApprovalRule {
                    approver_id: approver(),
                    accepts_device_trust: GrantDeviceTrust::Refused,
                },
            )
        } else {
            owner.issue_root(
                &agent("grant-root"),
                id("owner-delegation"),
                owner_agent(),
                terms.offering_subject.clone(),
                offering_bounds,
            )
        }
        .expect("the offering grant");
        let mut controller_bounds = ExecutionRestrictions::unrestricted();
        controller_bounds.delegation_depth = DelegationDepth::remaining(terms.controller_depth);
        let controller = owner
            .issue_root(
                &agent("grant-root"),
                id("data-agent-controller"),
                data_agent(),
                terms.controller_subject.clone(),
                controller_bounds,
            )
            .expect("the asking agent's controller grant");
        let branches = Arc::new(Branches::default());
        branches.can_branch(
            &data_agent(),
            ApprovalGranteeBasis::new(vec![controller.clone()], terms.controller_subject),
        );
        let route = Arc::new(Route::default());
        let custody: Arc<dyn ApprovalControllerCustody> =
            terms.custody.unwrap_or_else(|| Arc::new(Custody));
        let grants = compose(&decisions, &owner, &branches, &route, &custody, &clock);
        Self {
            clock,
            owner,
            decisions,
            branches,
            route,
            _custody: custody,
            grants,
            offering: vec![offering],
            controller: vec![controller],
            grant_store: terms.grant_store,
        }
    }

    /// The same host after a restart: the grant owner restored from its
    /// journal and the approval-carried grants composed again over the same
    /// decision store, branches, route and custody. Nothing of the stopped
    /// process's memory carries over, and the host's record of calls starts
    /// empty.
    fn restarted(self) -> Self {
        let Self {
            clock,
            owner,
            decisions,
            branches,
            route,
            _custody: custody,
            grants,
            offering,
            controller,
            grant_store,
        } = self;
        drop(grants);
        drop(owner);
        let grant_store = grant_store.expect("a durable grant owner");
        let owner = Arc::new(
            LocalGrantAuthority::restore(
                Arc::clone(&grant_store),
                LocalAuthorizationPublication::new(),
                clock.clone(),
            )
            .expect("the grant owner restored from its journal"),
        );
        branches.calls.lock().expect("calls").clear();
        let grants = compose(&decisions, &owner, &branches, &route, &custody, &clock);
        Self {
            clock,
            owner,
            decisions,
            branches,
            route,
            _custody: custody,
            grants,
            offering,
            controller,
            grant_store: Some(grant_store),
        }
    }

    /// The calling work of the owner's agent: in a peer work the data agent
    /// asked through (`peer`), else a work the requester asked directly. Its
    /// basis selects the offering grant for the server's operations.
    fn calling(&self, peer: bool) -> CallingWork {
        let basis = WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: vec![TargetGrantLineage {
                target: GrantTarget::McpServer {
                    server: SERVER.into(),
                },
                lineage: self.offering.clone(),
            }],
        };
        let association = association(peer, basis);
        let origin = GovernedPeerOrigin::from_associations([&association]).expect("origin");
        CallingWork::of(&origin).expect("the calling work")
    }

    fn offer(&self, peer: bool) -> Result<StoredDecision, AccessOfferError> {
        self.grants
            .offer(&self.calling(peer), arguments(1, 15 * MINUTE_MS))
    }

    /// Present the held offer to the approver and approve it (or decline).
    fn decide(&self, decision: &StoredDecision, approve: bool) -> StoredDecision {
        let rendering = &decision.descriptor.rendering;
        let at_ms = self.clock.now();
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
            AuthenticatedApprover::new(
                principal(PrincipalKind::Human, "the-data-owner"),
                ApprovalAssurance::PersonAuthenticated,
            )
            .expect("an authenticated person"),
            DecisionIngress::new("chat").expect("ingress"),
            reference("event-1"),
            at_ms + 1,
            approver(),
            rendering.subject_digest(),
            approve,
        )
        .expect("attested decision");
        self.decisions
            .decide(&decision.decision_id, &attested, &Eligible)
            .expect("decide")
    }

    fn executor_loop(&self) -> DecisionExecutorLoop {
        DecisionExecutorLoop::start(
            self.decisions.clone(),
            [self.grants.executor()],
            Arc::new(Approvers),
        )
        .expect("the executor loop")
    }

    fn stored(&self, decision: &DecisionId) -> StoredDecision {
        self.decisions.get(decision).expect("the stored decision")
    }

    fn issued(&self, id: Option<EvidenceId>) -> Option<GrantLineageRef> {
        self.owner
            .issued_reference(&id.expect("a derived id"))
            .expect("the grant owner answers")
    }

    /// The decision an offer of the default arguments in the data agent's
    /// peer work would be held under.
    fn offered_decision(&self) -> DecisionId {
        let operation_digest = meerkat_core::mcp_request_digest(
            SERVER,
            TOOL,
            &arguments(1, MINUTE_MS).operation.arguments,
        )
        .expect("digest");
        let subject = super::body::grant_child_subject_key(
            &id("owner-delegation"),
            &data_agent(),
            &data_agent_work(),
            &operation_digest,
        )
        .expect("subject key");
        DecisionId::derive(&super::offer::grant_child_kind().expect("kind"), &subject)
    }

    fn decision_count(&self) -> u64 {
        self.decisions
            .count(&meerkat_core::approval::decision::DecisionListFilter::default())
            .expect("count")
    }
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

fn realm() -> RealmId {
    RealmId::parse(DOMAIN).expect("realm")
}

fn ingress_domain() -> ResourceDomain {
    ResourceDomain {
        authority: host_ingress(),
        namespace: "works".into(),
    }
}

/// The data agent's own work for the requester: where a peer request comes
/// from.
fn data_agent_work() -> OriginalWorkRef {
    OriginalWorkRef {
        authority: host_ingress(),
        work: id("requester-question"),
    }
}

/// The owner's agent's work: a peer work the data agent sent (`peer`), or
/// the requester's own question to it.
fn association(peer: bool, basis: WorkAuthorityBasis) -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: requester(),
        ingress_actor: if peer { data_agent() } else { host_ingress() },
        represented_subject: None,
        original_authentication: evidence("authentication"),
        logical_executor: owner_agent(),
        target: NativeWorkTarget {
            logical_owner: agent("native-owner"),
            logical_runtime: id("owner-runtime"),
            context: id("owner-context"),
            context_generation: 1,
            audience: meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                principal: requester(),
            },
        },
        original_work: OriginalWorkRef {
            authority: host_ingress(),
            work: id(if peer {
                "peer-delivery"
            } else {
                "owner-question"
            }),
        },
        root_event: evidence("event"),
        contributing_work: if peer {
            vec![data_agent_work()]
        } else {
            Vec::new()
        },
        authority_basis: basis,
        controller_grant_lineage: Vec::new(),
        controller_model: None,
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: realm(),
            ingress: ingress_domain(),
            occurrence_scope: id("occurrence"),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .expect("the calling work's claims")
}

fn arguments(uses: u64, duration_ms: u64) -> AccessOfferArguments {
    AccessOfferArguments {
        operation: OperationArguments {
            server: SERVER.into(),
            tool: TOOL.into(),
            arguments: serde_json::json!({"query": "revenue by region, last quarter"}),
        },
        uses: Some(uses),
        duration_ms: Some(duration_ms),
        summary: "the requester asks for last quarter's revenue by region".into(),
    }
}

fn refusal(result: Result<StoredDecision, AccessOfferError>) -> AccessOfferRefusal {
    match result {
        Err(AccessOfferError::Refused(refusal)) => refusal,
        other => panic!("expected a refusal at hold, got {other:?}"),
    }
}

fn refused(
    kind: AccessOfferRefusalKind,
    reason: Option<AccessOfferRefusalReason>,
) -> AccessOfferRefusal {
    AccessOfferRefusal {
        refusal: kind,
        reason,
    }
}

#[test]
fn access_offer_refuses_without_a_parent_rule_a_reply_route_or_a_branch() {
    // No child rule on the offering grant.
    let fixture = Fixture::with(Terms {
        rule: false,
        ..Terms::default()
    });
    assert_eq!(
        refusal(fixture.offer(true)),
        refused(AccessOfferRefusalKind::NoChildApprovalRule, None)
    );
    assert_eq!(fixture.decision_count(), 0, "nothing was held");

    // No grant of the calling work covers the operation's server.
    let fixture = Fixture::new();
    let mut calling = fixture.calling(true);
    calling.basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: Some(fixture.offering.clone()),
        targets: Vec::new(),
    };
    assert_eq!(
        refusal(fixture.grants.offer(&calling, arguments(1, MINUTE_MS))),
        refused(
            AccessOfferRefusalKind::NoChildApprovalRule,
            Some(AccessOfferRefusalReason::NoParentGrant)
        ),
        "the base lineage is never the offering grant"
    );

    // The host cannot route a reply to the request's origin.
    fixture.route.deliverable.store(false, Ordering::SeqCst);
    assert_eq!(
        refusal(fixture.offer(true)),
        refused(AccessOfferRefusalKind::ReplyRouteUnavailable, None)
    );
    fixture.route.deliverable.store(true, Ordering::SeqCst);

    // The asking agent cannot be branched here.
    fixture.branches.bases.lock().expect("bases").clear();
    assert_eq!(
        refusal(fixture.offer(true)),
        refused(
            AccessOfferRefusalKind::ApprovalBranchUnavailable,
            Some(AccessOfferRefusalReason::NotBranchable)
        )
    );

    // More uses, or a longer window, than the host's policy allows.
    let fixture = Fixture::new();
    assert_eq!(
        refusal(
            fixture
                .grants
                .offer(&fixture.calling(true), arguments(2, MINUTE_MS))
        ),
        refused(AccessOfferRefusalKind::UsesOverPolicy, None)
    );
    assert_eq!(
        refusal(
            fixture
                .grants
                .offer(&fixture.calling(true), arguments(1, 16 * MINUTE_MS))
        ),
        refused(AccessOfferRefusalKind::WindowOverPolicy, None)
    );
    assert_eq!(fixture.decision_count(), 0, "nothing was held");
    assert!(fixture.branches.opened().is_empty());

    // The control: the same offer is held.
    let held = fixture.offer(true).expect("the offer is held");
    assert_eq!(held.state.phase, DecisionPhase::Open);
    assert_eq!(fixture.decision_count(), 1);
}

#[test]
fn access_offer_refuses_a_subject_mismatch_at_hold_and_issues_nothing() {
    let fixture = Fixture::with(Terms {
        offering_subject: Some(principal(PrincipalKind::Human, "data-owner-person")),
        controller_subject: None,
        ..Terms::default()
    });
    assert_eq!(
        refusal(fixture.offer(true)),
        refused(
            AccessOfferRefusalKind::ApprovalBranchUnavailable,
            Some(AccessOfferRefusalReason::SubjectMismatch)
        ),
        "an approved work has one represented subject"
    );
    assert_eq!(fixture.decision_count(), 0, "nothing was held");
    let decision = fixture.offered_decision();
    assert_eq!(fixture.issued(branch_controller_grant_id(&decision)), None);
    assert_eq!(fixture.issued(approval_grant_id(&decision)), None);
    assert!(fixture.branches.opened().is_empty(), "nothing was issued");
}

#[test]
fn access_offer_refuses_a_controller_grant_that_cannot_admit_the_branch() {
    // The asking agent's controller grant may not be delegated.
    let fixture = Fixture::with(Terms {
        controller_depth: 0,
        ..Terms::default()
    });
    assert_eq!(
        refusal(fixture.offer(true)),
        refused(
            AccessOfferRefusalKind::ApprovalBranchUnavailable,
            Some(AccessOfferRefusalReason::ControllerCannotAdmitChild)
        )
    );
    assert_eq!(fixture.decision_count(), 0);

    // Its processors bound does not admit the branch.
    let fixture = Fixture::new();
    let mut narrow = ExecutionRestrictions::unrestricted();
    narrow.delegation_depth = DelegationDepth::remaining(1);
    narrow.processors = ExactRestriction::exact([ProcessorRef::Principal {
        principal: data_agent(),
    }]);
    let narrow = fixture
        .owner
        .issue_root(
            &agent("grant-root"),
            id("data-agent-narrow-controller"),
            data_agent(),
            None,
            narrow,
        )
        .expect("a controller grant processed by the agent only");
    fixture
        .branches
        .can_branch(&data_agent(), ApprovalGranteeBasis::new(vec![narrow], None));
    assert_eq!(
        refusal(fixture.offer(true)),
        refused(
            AccessOfferRefusalKind::ApprovalBranchUnavailable,
            Some(AccessOfferRefusalReason::ControllerCannotAdmitChild)
        )
    );
    assert_eq!(fixture.decision_count(), 0);
}

#[test]
fn access_offer_takes_requester_and_origin_from_the_calling_work() {
    let fixture = Fixture::new();
    // A peer work: the requester is the peer work's, the origin is the work
    // the data agent asked from, and the data agent's branch would run it.
    let held = fixture.offer(true).expect("held");
    let request = GrantChildRequest::of(&held).expect("a grant.child request");
    assert_eq!(request.terms.requester, requester());
    assert_eq!(request.origin.requester, requester());
    assert_eq!(request.origin.work, data_agent_work());
    assert_eq!(request.origin.realm, realm());
    assert_eq!(request.origin.ingress, ingress_domain());
    assert_eq!(request.approval_branch_of, data_agent());
    assert_eq!(
        Some(request.terms.grantee.clone()),
        approval_branch_principal(&data_agent(), &held.decision_id)
    );
    assert_eq!(request.terms.issuer, owner_agent());
    assert_eq!(request.parent_lineage, fixture.offering);
    assert_eq!(
        held.descriptor.requester,
        meerkat_core::approval::decision::DecisionPrincipalKey::of(&requester()).expect("key"),
        "the person who asked is the decision's requester, so never its approver"
    );
    assert_eq!(held.descriptor.approver_id, approver());
    assert_eq!(
        held.descriptor.operation_id.as_str(),
        format!("grant.child:{}", held.decision_id)
    );
    assert_eq!(
        request.operation.request_digest,
        meerkat_core::mcp_request_digest(SERVER, TOOL, &request.operation.arguments)
            .expect("digest"),
        "the operation's digest is computed from its exact arguments"
    );
    // The work the requester asked the owner's agent directly: the origin is
    // that work, and its own branch would run the approved work, under a
    // child of its own controller grant.
    let fixture = Fixture::new();
    let mut delegable = ExecutionRestrictions::unrestricted();
    delegable.delegation_depth = DelegationDepth::remaining(1);
    let owner_controller = fixture
        .owner
        .issue_root(
            &agent("grant-root"),
            id("owner-agent-controller"),
            owner_agent(),
            None,
            delegable,
        )
        .expect("the owner's agent's controller grant");
    fixture.branches.can_branch(
        &owner_agent(),
        ApprovalGranteeBasis::new(vec![owner_controller], None),
    );
    let held = fixture.offer(false).expect("held");
    let request = GrantChildRequest::of(&held).expect("a grant.child request");
    assert_eq!(request.approval_branch_of, owner_agent());
    assert_eq!(request.origin.work.work, id("owner-question"));
    // The data agent's controller grant cannot stand in for the owner's
    // agent's: the branch's controller grant is a child of the grantee's own.
    let fixture = Fixture::new();
    fixture.branches.can_branch(
        &owner_agent(),
        ApprovalGranteeBasis::new(fixture.controller.clone(), None),
    );
    assert_eq!(
        refusal(fixture.offer(false)),
        refused(
            AccessOfferRefusalKind::ApprovalBranchUnavailable,
            Some(AccessOfferRefusalReason::ControllerCannotAdmitChild)
        )
    );
}

#[tokio::test]
async fn the_executor_closes_terms_changed_when_the_parent_narrowed() {
    let start = real_now_ms();
    let fixture = Fixture::with(Terms {
        offering_lifetime: Some(LifetimeRestriction::window(start, start + 20 * MINUTE_MS)),
        ..Terms::default()
    });
    let held = fixture.offer(true).expect("held");
    fixture.decide(&held, true);
    // The offering grant's window now ends before the approved window would:
    // the effective terms are no longer the presented ones.
    fixture.clock.advance(10 * MINUTE_MS);
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes.contains(&(
            held.decision_id.clone(),
            DecisionPassOutcome::ExecutionRefused
        )),
        "{pass:?}"
    );
    let stored = fixture.stored(&held.decision_id);
    assert_eq!(stored.state.phase, DecisionPhase::ExecutionRefused);
    assert_eq!(
        stored
            .execution_refusal
            .as_ref()
            .map(DecisionReference::as_str),
        Some("terms_changed")
    );
    assert_eq!(fixture.issued(approval_grant_id(&held.decision_id)), None);
    assert!(fixture.branches.opened().is_empty(), "no branch was opened");
}

#[tokio::test]
async fn the_executor_settles_a_landed_issue_first_after_a_restart() {
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    let approved = fixture.decide(&held, true);
    // An earlier executor authorized and issued, then stopped before it
    // recorded the decision executed.
    let permit = fixture
        .decisions
        .authorize_execution(
            &approved.decision_id,
            &approved.descriptor.operation_id,
            &approved.descriptor.request_digest,
            &ConsentEligibilityAnswer::Eligible(Arc::new(Eligible)),
        )
        .expect("a permit");
    let execution = GrantDecisionExecutor::new(fixture.grants.clone())
        .execute(&fixture.stored(&approved.decision_id), &permit)
        .await;
    assert_eq!(execution, crate::decision::DecisionExecution::Committed);
    drop(permit);
    let issued = fixture
        .issued(approval_grant_id(&held.decision_id))
        .expect("the approval-carried grant landed");
    // The next executor settles that commit first, and issues nothing more.
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes.contains(&(
            held.decision_id.clone(),
            DecisionPassOutcome::SettledExecuted
        )),
        "{pass:?}"
    );
    assert_eq!(
        fixture.stored(&held.decision_id).state.phase,
        DecisionPhase::Executed
    );
    assert_eq!(
        fixture.issued(approval_grant_id(&held.decision_id)),
        Some(issued),
        "the landed grant is the decision's, issued once"
    );
    assert_eq!(
        fixture.branches.opened().len(),
        1,
        "no branch was opened again"
    );
}

#[tokio::test]
async fn the_branch_controller_grant_is_issued_and_recovered_on_retry() {
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    fixture.decide(&held, true);
    let branch =
        approval_branch_principal(&data_agent(), &held.decision_id).expect("branch principal");
    // An earlier attempt issued the branch's controller grant, then stopped.
    let intent = branch_controller_intent(
        &held.decision_id,
        &data_agent(),
        &ApprovalGranteeBasis::new(fixture.controller.clone(), None),
        &branch,
    )
    .expect("the intent");
    let validity = ChildGrantValidity {
        not_before_ms: 0,
        expires_at_ms: u64::MAX,
        deadline: Instant::now() + std::time::Duration::from_mins(1),
    };
    let ChildGrantOutcome::Issued { lineage: first } = fixture
        .owner
        .issue_or_recover_child_under_exclusion(&intent, || Ok(validity))
        .expect("the earlier attempt's issuance")
    else {
        panic!("the first attempt issues the branch's controller grant");
    };
    // The retry recovers it under the id the decision derives, and issues
    // the approval-carried grant.
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes
            .contains(&(held.decision_id.clone(), DecisionPassOutcome::Executed)),
        "{pass:?}"
    );
    let controller = fixture
        .issued(branch_controller_grant_id(&held.decision_id))
        .expect("the branch's controller grant");
    assert_eq!(
        first.last(),
        Some(&controller),
        "recovered, never issued twice"
    );
    // The controller grant is the branch's: depth 0, processed by the branch
    // only, a controller's unrestricted lifetime.
    let resolved = fixture
        .owner
        .resolve_controller_lineage(&first, &branch, None)
        .expect("a current controller lineage of the branch");
    assert_eq!(
        resolved.restrictions().processors,
        ExactRestriction::exact([ProcessorRef::Principal {
            principal: branch.clone()
        }])
    );
    assert!(
        fixture
            .issued(approval_grant_id(&held.decision_id))
            .is_some()
    );
    assert_eq!(fixture.branches.opened().len(), 1);
    assert_eq!(fixture.branches.opened()[0].branch, branch);
}

#[tokio::test]
async fn the_branch_controller_grant_is_revoked_when_the_branch_retires() {
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    fixture.decide(&held, true);
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes
            .contains(&(held.decision_id.clone(), DecisionPassOutcome::Executed)),
        "{pass:?}"
    );
    let branch =
        approval_branch_principal(&data_agent(), &held.decision_id).expect("branch principal");
    let controller = fixture
        .issued(branch_controller_grant_id(&held.decision_id))
        .expect("the branch's controller grant");
    let mut lineage = fixture.controller.clone();
    lineage.push(controller);
    fixture
        .owner
        .resolve_controller_lineage(&lineage, &branch, None)
        .expect("current while the branch runs");
    fixture
        .grants
        .retire_branch(&held.decision_id, &data_agent())
        .await;
    assert_eq!(fixture.branches.retired(), vec![held.decision_id.clone()]);
    assert!(
        fixture
            .owner
            .resolve_controller_lineage(&lineage, &branch, None)
            .is_err(),
        "revoked once the branch retired"
    );
}

#[tokio::test]
async fn an_ended_offer_tells_its_origin_with_no_content_and_retires_its_branch() {
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    fixture.decide(&held, false);
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes
            .contains(&(held.decision_id.clone(), DecisionPassOutcome::NoticeSettled)),
        "{pass:?}"
    );
    assert_eq!(
        *fixture.route.notices.lock().expect("notices"),
        vec![(held.decision_id.clone(), AccessOutcome::Declined)]
    );
    assert_eq!(fixture.branches.retired(), vec![held.decision_id.clone()]);
    assert_eq!(fixture.issued(approval_grant_id(&held.decision_id)), None);
}

#[tokio::test]
async fn an_open_branch_past_its_window_is_retired_by_the_sweep() {
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    fixture.decide(&held, true);
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes
            .contains(&(held.decision_id.clone(), DecisionPassOutcome::Executed)),
        "{pass:?}"
    );
    // Within its window the branch stays open: its work may still run.
    assert_eq!(fixture.grants.sweep_branches().await, 0);
    assert!(fixture.branches.retired().is_empty());
    // Past its window (a restart ended its watcher), the sweep retires it and
    // revokes its controller grant.
    fixture.clock.advance(15 * MINUTE_MS + 1);
    assert_eq!(fixture.grants.sweep_branches().await, 1);
    assert_eq!(fixture.branches.retired(), vec![held.decision_id.clone()]);
    let branch =
        approval_branch_principal(&data_agent(), &held.decision_id).expect("branch principal");
    let mut lineage = fixture.controller.clone();
    lineage.push(
        fixture
            .issued(branch_controller_grant_id(&held.decision_id))
            .expect("the branch's controller grant"),
    );
    assert!(
        fixture
            .owner
            .resolve_controller_lineage(&lineage, &branch, None)
            .is_err()
    );
    assert_eq!(
        fixture.grants.sweep_branches().await,
        0,
        "nothing left open"
    );
}

#[test]
fn a_held_request_is_read_only_as_core_held_it() {
    let fixture = Fixture::new();
    let held = fixture.offer(true).expect("held");
    assert!(GrantChildRequest::of(&held).is_ok());
    // Another rendering than the one Core renders of the request is not
    // what the approver must see, so it never executes.
    let mut forged = held.clone();
    forged.descriptor.rendering =
        meerkat_core::approval::decision::DecisionRendering::new("Approve a harmless read")
            .expect("rendering");
    assert!(GrantChildRequest::of(&forged).is_err());
    // Nor another subject or operation id than the request derives.
    let mut forged = held.clone();
    forged.descriptor.subject_key =
        meerkat_core::approval::decision::DecisionSubjectKey::new("another-subject")
            .expect("subject key");
    assert!(GrantChildRequest::of(&forged).is_err());
    let mut forged = held.clone();
    forged.descriptor.operation_id =
        meerkat_core::approval::decision::DecisionOperationId::new("grant.child:other")
            .expect("operation id");
    assert!(GrantChildRequest::of(&forged).is_err());
    // A body held by another path, consistent with itself: its terms digest,
    // subject key and rendering are the ones Core derives from it. The
    // control is Core's own body, held again so: it is read.
    assert!(GrantChildRequest::of(&reheld(&held, |_| {})).is_ok());
    // It shows one query and binds the digest of another: the digest is not
    // the digest of the arguments the approver saw.
    let forged = reheld(&held, |request| {
        request.operation.arguments = serde_json::json!({"query": "every customer's card numbers"});
    });
    let refused = GrantChildRequest::of(&forged)
        .expect_err("a request digest that is not the shown arguments' is refused");
    assert!(refused.to_string().contains("request digest"), "{refused}");
    // Its reply goes to another person than the terms' requester.
    let forged = reheld(&held, |request| {
        request.origin.requester = principal(PrincipalKind::Human, "someone-else");
    });
    let refused =
        GrantChildRequest::of(&forged).expect_err("a reply origin of another requester is refused");
    assert!(refused.to_string().contains("requester"), "{refused}");
    // Its terms name another work than the decision's approved work.
    let forged = reheld(&held, |request| {
        request.terms.work = OriginalWorkRef {
            authority: host_ingress(),
            work: id("another-work"),
        };
    });
    let refused = GrantChildRequest::of(&forged)
        .expect_err("terms of another work than the approved work are refused");
    assert!(refused.to_string().contains("approved work"), "{refused}");
}

/// `held` with its request body edited by `edit`, held as consistently as a
/// path other than `access_offer` could: the terms digest, the subject key
/// and the rendering are the ones Core derives from the edited body.
fn reheld(held: &StoredDecision, edit: impl FnOnce(&mut GrantChildRequest)) -> StoredDecision {
    let mut request = GrantChildRequest::of(held).expect("Core's own request");
    edit(&mut request);
    let mut forged = held.clone();
    forged.descriptor.request = request.body().expect("a request body");
    forged.descriptor.request_digest =
        meerkat_authorization::grants::grant_child_terms_digest(&request.terms)
            .expect("the terms digest");
    forged.descriptor.subject_key = super::body::grant_child_subject_key(
        &request.parent_lineage.last().expect("a parent").grant_id,
        &request.approval_branch_of,
        &request.origin.work,
        &request.operation.request_digest,
    )
    .expect("a subject key");
    forged.descriptor.rendering =
        super::body::render(&request, &forged.descriptor.approver_id).expect("a rendering");
    forged
}

/// An offer of the data agent's peer work, approved by a person and executed
/// by the loop's pass: the branch is opened and both grants are issued.
async fn executed(fixture: &Fixture) -> DecisionId {
    let held = fixture.offer(true).expect("held");
    fixture.decide(&held, true);
    let pass = fixture.executor_loop().pass().await;
    assert!(
        pass.outcomes
            .contains(&(held.decision_id.clone(), DecisionPassOutcome::Executed)),
        "{pass:?}"
    );
    held.decision_id
}

/// Settle the Executed notice `decision` owes, as its delivery once did:
/// these rows' host seats nothing, so its notice would stay owed.
fn settle_the_notice(fixture: &Fixture, decision: &DecisionId) {
    fixture
        .decisions
        .settle_notice(
            decision,
            &ApprovalConsentDeliveryState::Failed {
                reason: "settled by the row".into(),
            },
        )
        .expect("the notice settles");
}

/// The branch's controller lineage: the asking agent's controller grant and
/// the branch's own, and the branch principal it serves.
fn branch_controller_lineage(
    fixture: &Fixture,
    decision: &DecisionId,
) -> (Vec<GrantLineageRef>, PrincipalRef) {
    let mut lineage = fixture.controller.clone();
    lineage.push(
        fixture
            .issued(branch_controller_grant_id(decision))
            .expect("the branch's controller grant"),
    );
    (
        lineage,
        approval_branch_principal(&data_agent(), decision).expect("branch principal"),
    )
}

/// Whether the branch's controller lineage still resolves: its controller
/// grant is not revoked.
fn controller_current(fixture: &Fixture, decision: &DecisionId) -> bool {
    let (lineage, branch) = branch_controller_lineage(fixture, decision);
    fixture
        .owner
        .resolve_controller_lineage(&lineage, &branch, None)
        .is_ok()
}

mod binding;
mod cleanup;
