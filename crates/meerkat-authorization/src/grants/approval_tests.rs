//! Approval-carried children and the spend of their uses, through the real
//! generated owner: issuance under a parent's child rule, the terms of the
//! approved request, one grant per decision, uses bound to one requester,
//! one work and one operation, the spend before entry and its journal.
#![allow(clippy::expect_used, clippy::panic)]

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::clock::{LocalAuthorizationTime, LocalClockError};
use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, DelegationDepth, ExactRestriction, LifetimeRestriction, ProcessorRef,
};
use meerkat_authorization_contracts::work_association::{GrantTarget, OriginalWorkRef};
use meerkat_core::approval::decision::{
    ApproverId, DecisionId, DecisionKind, DecisionSubjectKey, Sha256Digest,
};
use meerkat_core::time_compat::Instant;
use meerkat_core::{PrincipalKind, TrustDomainId};

const NOW_MS: u64 = 1_000;
const WINDOW_MS: u64 = 900_000;

struct Clock(AtomicU64);
impl LocalAuthorizationClock for Clock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        Ok(LocalAuthorizationTime {
            unix_ms: self.0.load(Ordering::SeqCst),
            monotonic: Instant::now(),
        })
    }
}

fn who(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("approval-grants").expect("domain"),
    )
    .expect("principal")
}
fn id(name: &str) -> EvidenceId {
    EvidenceId::new(name).expect("id")
}
fn owner_approver() -> ApproverId {
    ApproverId::new("data-owner").expect("approver id")
}
fn decision(name: &str) -> DecisionId {
    DecisionId::derive(
        &DecisionKind::new("grant.child").expect("kind"),
        &DecisionSubjectKey::new(name).expect("subject key"),
    )
}
fn work(name: &str) -> OriginalWorkRef {
    OriginalWorkRef {
        authority: who("decision-ingress"),
        work: id(name),
    }
}
fn action(name: &str) -> ActionRef {
    ActionRef {
        feature: "records".into(),
        action: name.into(),
    }
}
fn operation(request: &str) -> GrantOperationBinding {
    GrantOperationBinding {
        target: GrantTarget::McpServer {
            server: "records".into(),
        },
        action: action("query"),
        request_digest: Sha256Digest::of(request.as_bytes()),
    }
}
fn rule(device_trust: GrantDeviceTrust) -> ChildApprovalRule {
    ChildApprovalRule {
        approver_id: owner_approver(),
        accepts_device_trust: device_trust,
    }
}
/// The owner's delegation to its agent: its records, one more level.
fn delegation(depth: u32) -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact([action("query"), action("list")]),
        delegation_depth: DelegationDepth::remaining(depth),
        ..ExecutionRestrictions::unrestricted()
    }
}
/// The offered terms: one action, the branch as processor, the requester as
/// audience, a 15-minute window from now, no further delegation.
fn offered() -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact([action("query")]),
        processors: ExactRestriction::exact([ProcessorRef::Principal {
            principal: who("branch"),
        }]),
        audiences: ExactRestriction::exact([AudienceRef::Principal {
            principal: who("requester"),
        }]),
        lifetime: LifetimeRestriction::window(NOW_MS, NOW_MS + WINDOW_MS),
        delegation_depth: DelegationDepth::remaining(0),
        ..ExecutionRestrictions::unrestricted()
    }
}

/// A grant custody that vetoes nothing: no grant revoked here is a
/// controller lineage in use.
struct NoControllerCustody;
impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
    for NoControllerCustody
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

struct Fixture {
    authority: LocalGrantAuthority,
}

impl Fixture {
    fn new() -> Self {
        Self {
            authority: LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: who("root"),
                    namespace: id("namespace"),
                    generation: 1,
                },
                LocalAuthorizationPublication::new(),
                Arc::new(Clock(AtomicU64::new(NOW_MS))),
            )
            .expect("configure"),
        }
    }

    fn durable(store: Arc<dyn GrantOwnerStore>) -> Self {
        Self::durable_under(&AdmissionGuard::none(), store)
    }

    /// A durable owner over `store`, created under `guard`.
    fn durable_under(guard: &AdmissionGuard, store: Arc<dyn GrantOwnerStore>) -> Self {
        Self {
            authority: LocalGrantAuthority::new_durable(
                guard,
                LocalGrantConfiguration {
                    root: who("root"),
                    namespace: id("namespace"),
                    generation: 1,
                },
                store,
                LocalAuthorizationPublication::new(),
                Arc::new(Clock(AtomicU64::new(NOW_MS))),
            )
            .expect("durable owner"),
        }
    }

    /// A durable owner over the realm grant log `log`, created under its
    /// fence, which is released when this returns.
    fn fenced(log: &FencedLog) -> Self {
        Self::durable_under(&log.guard(), log.store())
    }

    fn restored(store: Arc<dyn GrantOwnerStore>) -> Self {
        Self {
            authority: LocalGrantAuthority::restore(
                store,
                LocalAuthorizationPublication::new(),
                Arc::new(Clock(AtomicU64::new(NOW_MS))),
            )
            .expect("restored owner"),
        }
    }

    /// The owner's delegation to its agent, under the owner's child rule.
    fn ruled_parent(&self, rule: ChildApprovalRule) -> GrantLineageRef {
        self.try_ruled_parent(&AdmissionGuard::none(), rule)
            .expect("ruled parent")
    }

    /// [`Self::ruled_parent`], appended under `guard`.
    fn try_ruled_parent(
        &self,
        guard: &AdmissionGuard,
        rule: ChildApprovalRule,
    ) -> Result<GrantLineageRef, GrantRefusal> {
        self.authority.issue_root_with_child_rule(
            guard,
            &who("root"),
            id("owner-delegation"),
            who("owner-agent"),
            None,
            delegation(1),
            rule,
        )
    }

    /// The same delegation without a child rule.
    fn plain_parent(&self) -> GrantLineageRef {
        self.authority
            .issue_root(
                &AdmissionGuard::none(),
                &who("root"),
                id("plain-delegation"),
                who("owner-agent"),
                None,
                delegation(1),
            )
            .expect("plain parent")
    }

    fn record(&self, reference: &GrantLineageRef) -> GrantRecord {
        let owner = self.authority.owner.lock().expect("owner");
        exact_record(&owner, reference).expect("retained").clone()
    }

    /// The approval the owner's decision `name` carries for the offered
    /// terms, its request digest the digest of exactly those terms.
    fn approval(&self, parent: &GrantLineageRef, name: &str) -> GrantApproval {
        let mut approval = GrantApproval {
            decision_id: decision(name),
            approver_id: owner_approver(),
            tier: GrantDecisionTier::Human,
            assurance: GrantDecisionAssurance::PersonAuthenticated,
            request_digest: Sha256Digest::of(b"placeholder"),
            requester: GrantPrincipal::new(who("requester")).expect("requester"),
            work: work("approved-work-1"),
            operation: None,
            uses: 1,
            review_before_return: None,
        };
        approval.request_digest = self.terms_digest(parent, &offered(), &approval);
        approval
    }

    /// The digest of the terms `approval` would be issued with under
    /// `parent`, for `requested`.
    fn terms_digest(
        &self,
        parent: &GrantLineageRef,
        requested: &ExecutionRestrictions,
        approval: &GrantApproval,
    ) -> Sha256Digest {
        let parent = self.record(parent);
        let derived = DerivedChildRestrictions::new(parent.restrictions.clone(), requested.clone())
            .expect("attenuation");
        let record = GrantRecord {
            id: id("terms"),
            authority_incarnation: parent.authority_incarnation,
            parent: Some(parent.id.clone()),
            issuer: parent.grantee.clone(),
            grantee: GrantPrincipal::new(who("branch")).expect("branch"),
            represented_subject: parent.represented_subject,
            issued_revision: 0,
            restrictions: derived.effective().clone(),
            child_approval: None,
            approval: Some(approval.clone()),
        };
        grant_child_terms_digest(&GrantChildTerms::of_record(&record).expect("terms"))
            .expect("digest")
    }

    fn issue(
        &self,
        parent: &GrantLineageRef,
        child: &str,
        approval: GrantApproval,
    ) -> Result<GrantLineageRef, ApprovalChildRefusal> {
        self.issue_under(&AdmissionGuard::none(), parent, child, approval)
    }

    /// [`Self::issue`], appended under `guard`.
    fn issue_under(
        &self,
        guard: &AdmissionGuard,
        parent: &GrantLineageRef,
        child: &str,
        approval: GrantApproval,
    ) -> Result<GrantLineageRef, ApprovalChildRefusal> {
        self.authority.issue_approval_child(
            guard,
            &who("owner-agent"),
            parent,
            id(child),
            who("branch"),
            offered(),
            approval,
        )
    }

    /// Apply the generated owner's own `IssueChild` for `approval` under
    /// `parent`, bypassing every check of the shell: whether the machine
    /// itself accepts it.
    fn machine_accepts_child(&self, parent: &GrantLineageRef, approval: GrantApproval) -> bool {
        let mut owner = self.authority.owner.lock().expect("owner");
        let parent = exact_record(&owner, parent).expect("parent").clone();
        let chain = chain(&owner, &parent).expect("chain");
        let derived = DerivedChildRestrictions::new(parent.restrictions.clone(), offered())
            .expect("attenuation");
        let record = GrantRecord {
            id: id("direct-child"),
            authority_incarnation: parent.authority_incarnation,
            parent: Some(parent.id.clone()),
            issuer: parent.grantee.clone(),
            grantee: GrantPrincipal::new(who("branch")).expect("branch"),
            represented_subject: parent.represented_subject.clone(),
            issued_revision: owner.state().revision + 1,
            restrictions: derived.effective().clone(),
            child_approval: None,
            approval: Some(approval),
        };
        let terms_digest = approval::issued_terms_digest(&record).expect("terms digest");
        let before = owner.state().clone();
        let accepted = owner
            .apply(dsl::GrantAuthorityInput::IssueChild {
                actor: parent.grantee,
                record,
                derived,
                chain,
                now_ms: NOW_MS,
                terms_digest,
            })
            .is_ok();
        if !accepted {
            assert!(
                owner.state() == &before,
                "a refused issuance changes nothing"
            );
        }
        accepted
    }
}

fn the_use<'a>(
    requester: &'a PrincipalRef,
    work: &'a OriginalWorkRef,
    operation: Option<&'a GrantOperationBinding>,
) -> GrantUse<'a> {
    GrantUse {
        requester,
        work,
        operation,
    }
}

/// The approved child's lineage, resolved for its own use.
fn resolve_own(
    fixture: &Fixture,
    lineage: &[GrantLineageRef],
    operation: Option<&GrantOperationBinding>,
) -> Result<ResolvedGrant, GrantRefusal> {
    let requester = who("requester");
    let approved = work("approved-work-1");
    fixture.authority.resolve_lineage_with_use(
        lineage,
        &who("branch"),
        None,
        the_use(&requester, &approved, operation),
    )
}

fn consume_own(
    fixture: &Fixture,
    lineage: &[GrantLineageRef],
    approver_current: bool,
) -> Result<(), ConsumeUseRefusal> {
    consume_own_under(fixture, &AdmissionGuard::none(), lineage, approver_current)
}

/// [`consume_own`], appended under `guard`.
fn consume_own_under(
    fixture: &Fixture,
    guard: &AdmissionGuard,
    lineage: &[GrantLineageRef],
    approver_current: bool,
) -> Result<(), ConsumeUseRefusal> {
    let requester = who("requester");
    let approved = work("approved-work-1");
    fixture.authority.consume_use(
        guard,
        lineage,
        &who("branch"),
        None,
        the_use(&requester, &approved, None),
        approver_current,
    )
}

fn approved_child(fixture: &Fixture, parent: &GrantLineageRef) -> Vec<GrantLineageRef> {
    let approval = fixture.approval(parent, "decision-1");
    let child = fixture
        .issue(parent, "approval-child", approval)
        .expect("approval child");
    vec![parent.clone(), child]
}

#[test]
fn a_child_without_the_parents_approvers_decision_is_refused() {
    let fixture = Fixture::new();
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    // A plain child of a grant under a rule: no decision at all.
    assert_eq!(
        fixture.authority.issue_child(
            &AdmissionGuard::none(),
            &who("owner-agent"),
            &parent,
            id("plain-child"),
            who("branch"),
            offered(),
        ),
        Err(GrantRefusal::Denied)
    );
    // A decision for another approver id.
    let mut other = fixture.approval(&parent, "decision-1");
    other.approver_id = ApproverId::new("someone-else").expect("approver id");
    other.request_digest = fixture.terms_digest(&parent, &offered(), &other);
    assert_eq!(
        fixture.issue(&parent, "approval-child", other.clone()),
        Err(ApprovalChildRefusal::BelowRule)
    );
    assert!(!fixture.machine_accepts_child(&parent, other));
    // A reviewer's verdict, not a person's decision.
    let mut reviewed = fixture.approval(&parent, "decision-1");
    reviewed.tier = GrantDecisionTier::Review;
    reviewed.assurance = GrantDecisionAssurance::Unattested;
    reviewed.request_digest = fixture.terms_digest(&parent, &offered(), &reviewed);
    assert_eq!(
        fixture.issue(&parent, "approval-child", reviewed.clone()),
        Err(ApprovalChildRefusal::BelowRule)
    );
    assert!(!fixture.machine_accepts_child(&parent, reviewed));
    // The owner's own decision is issued.
    let approval = fixture.approval(&parent, "decision-1");
    assert!(fixture.issue(&parent, "approval-child", approval).is_ok());
}

#[test]
fn an_approval_child_without_a_parent_rule_is_refused() {
    let fixture = Fixture::new();
    let parent = fixture.plain_parent();
    let approval = fixture.approval(&parent, "decision-1");
    assert_eq!(
        fixture.issue(&parent, "approval-child", approval.clone()),
        Err(ApprovalChildRefusal::NoChildRule)
    );
    assert!(!fixture.machine_accepts_child(&parent, approval));
    // A plain child of the plain parent is issued as before.
    assert!(
        fixture
            .authority
            .issue_child(
                &AdmissionGuard::none(),
                &who("owner-agent"),
                &parent,
                id("plain-child"),
                who("branch"),
                offered(),
            )
            .is_ok()
    );
}

#[test]
fn operator_device_trusted_decision_cannot_issue_a_child_under_an_owner_rule() {
    let fixture = Fixture::new();
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    let mut device = fixture.approval(&parent, "decision-1");
    device.assurance = GrantDecisionAssurance::DeviceTrusted;
    device.request_digest = fixture.terms_digest(&parent, &offered(), &device);
    assert_eq!(
        fixture.issue(&parent, "approval-child", device.clone()),
        Err(ApprovalChildRefusal::BelowRule)
    );
    assert!(!fixture.machine_accepts_child(&parent, device));
    // Under a rule that accepts device trust, the same decision is issued.
    let accepting = Fixture::new();
    let parent = accepting.ruled_parent(rule(GrantDeviceTrust::Accepted));
    let mut device = accepting.approval(&parent, "decision-1");
    device.assurance = GrantDecisionAssurance::DeviceTrusted;
    device.request_digest = accepting.terms_digest(&parent, &offered(), &device);
    assert!(accepting.issue(&parent, "approval-child", device).is_ok());
}

#[test]
fn service_credential_relay_is_refused_under_an_owner_rule() {
    for device_trust in [GrantDeviceTrust::Refused, GrantDeviceTrust::Accepted] {
        let fixture = Fixture::new();
        let parent = fixture.ruled_parent(rule(device_trust));
        let mut relayed = fixture.approval(&parent, "decision-1");
        relayed.assurance = GrantDecisionAssurance::ServiceCredential;
        relayed.request_digest = fixture.terms_digest(&parent, &offered(), &relayed);
        assert_eq!(
            fixture.issue(&parent, "approval-child", relayed.clone()),
            Err(ApprovalChildRefusal::BelowRule)
        );
        assert!(!fixture.machine_accepts_child(&parent, relayed));
    }
}

#[test]
fn issued_terms_must_equal_the_approved_request() {
    let fixture = Fixture::new();
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    // The approved request asked for one use; the issuance names two.
    let mut more_uses = fixture.approval(&parent, "decision-1");
    more_uses.uses = 2;
    assert_eq!(
        fixture.issue(&parent, "approval-child", more_uses.clone()),
        Err(ApprovalChildRefusal::TermsDiffer)
    );
    assert!(!fixture.machine_accepts_child(&parent, more_uses));
    // The approved request named another work.
    let mut other_work = fixture.approval(&parent, "decision-1");
    other_work.work = work("another-work");
    assert_eq!(
        fixture.issue(&parent, "approval-child", other_work.clone()),
        Err(ApprovalChildRefusal::TermsDiffer)
    );
    assert!(!fixture.machine_accepts_child(&parent, other_work));
    // The approved request was for a longer window.
    let mut longer = offered();
    longer.lifetime = LifetimeRestriction::window(NOW_MS, NOW_MS + 2 * WINDOW_MS);
    let mut approval = fixture.approval(&parent, "decision-1");
    approval.request_digest = fixture.terms_digest(&parent, &longer, &approval);
    assert_eq!(
        fixture.issue(&parent, "approval-child", approval.clone()),
        Err(ApprovalChildRefusal::TermsDiffer)
    );
    assert!(!fixture.machine_accepts_child(&parent, approval));
    // Exactly the approved terms are issued.
    let approval = fixture.approval(&parent, "decision-1");
    assert!(fixture.issue(&parent, "approval-child", approval).is_ok());
}

#[test]
fn an_approval_child_must_have_depth_zero_a_window_and_uses() {
    // A parent that could delegate two more levels: the child asks for one.
    let fixture = Fixture::new();
    let parent = fixture
        .authority
        .issue_root_with_child_rule(
            &AdmissionGuard::none(),
            &who("root"),
            id("deep-delegation"),
            who("owner-agent"),
            None,
            delegation(3),
            rule(GrantDeviceTrust::Refused),
        )
        .expect("deep parent");
    let mut delegable = offered();
    delegable.delegation_depth = DelegationDepth::remaining(1);
    let mut approval = fixture.approval(&parent, "decision-1");
    approval.request_digest = fixture.terms_digest(&parent, &delegable, &approval);
    assert_eq!(
        fixture.authority.issue_approval_child(
            &AdmissionGuard::none(),
            &who("owner-agent"),
            &parent,
            id("approval-child"),
            who("branch"),
            delegable,
            approval,
        ),
        Err(ApprovalChildRefusal::Unbounded)
    );
    // No window.
    let mut unbounded = offered();
    unbounded.lifetime = LifetimeRestriction::unrestricted();
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    let approval = fixture.approval(&parent, "decision-2");
    assert_eq!(
        fixture.authority.issue_approval_child(
            &AdmissionGuard::none(),
            &who("owner-agent"),
            &parent,
            id("approval-child"),
            who("branch"),
            unbounded,
            approval,
        ),
        Err(ApprovalChildRefusal::Unbounded)
    );
    // No use.
    let mut no_use = fixture.approval(&parent, "decision-3");
    no_use.uses = 0;
    no_use.request_digest = fixture.terms_digest(&parent, &offered(), &no_use);
    assert_eq!(
        fixture.issue(&parent, "approval-child", no_use.clone()),
        Err(ApprovalChildRefusal::Unbounded)
    );
    assert!(!fixture.machine_accepts_child(&parent, no_use));
}

#[test]
fn one_decision_issues_at_most_one_grant() {
    let fixture = Fixture::new();
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    let approval = fixture.approval(&parent, "decision-1");
    fixture
        .issue(&parent, "approval-child", approval.clone())
        .expect("the decision's grant");
    // The same id again: the grant this owner already issued.
    assert_eq!(
        fixture.issue(&parent, "approval-child", approval.clone()),
        Err(ApprovalChildRefusal::AlreadyIssued)
    );
    // Another id for the same decision.
    assert_eq!(
        fixture.issue(&parent, "approval-child-2", approval.clone()),
        Err(ApprovalChildRefusal::DecisionCarried)
    );
    assert!(!fixture.machine_accepts_child(&parent, approval));
    // Another decision is its own grant.
    let other = fixture.approval(&parent, "decision-2");
    assert!(fixture.issue(&parent, "approval-child-2", other).is_ok());
}

#[test]
fn consume_use_spends_once_and_advances_the_revision() {
    let fixture = Fixture::new();
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    let lineage = approved_child(&fixture, &parent);
    resolve_own(&fixture, &lineage, None).expect("an unspent use resolves");
    let revision = fixture
        .authority
        .owner
        .lock()
        .expect("owner")
        .state()
        .revision;
    consume_own(&fixture, &lineage, true).expect("the one use is spent");
    let state = fixture
        .authority
        .owner
        .lock()
        .expect("owner")
        .state()
        .clone();
    assert_eq!(state.revision, revision + 1);
    assert_eq!(state.consumed, 1);
    assert_eq!(state.spent.get(&lineage[1].grant_id), Some(&1));
    assert_eq!(
        consume_own(&fixture, &lineage, true),
        Err(ConsumeUseRefusal::Denied)
    );
    assert!(resolve_own(&fixture, &lineage, None).is_err());
    // A plain lineage spends nothing: there is no use to count.
    let plain = fixture
        .authority
        .issue_root(
            &AdmissionGuard::none(),
            &who("root"),
            id("plain"),
            who("branch"),
            None,
            offered(),
        )
        .expect("plain grant");
    assert_eq!(
        consume_own(&fixture, &[plain], true),
        Err(ConsumeUseRefusal::Denied)
    );
}

#[test]
fn a_revoked_approver_blocks_the_use_and_spends_nothing() {
    let fixture = Fixture::new();
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    let lineage = approved_child(&fixture, &parent);
    let before = fixture
        .authority
        .owner
        .lock()
        .expect("owner")
        .state()
        .clone();
    assert_eq!(
        consume_own(&fixture, &lineage, false),
        Err(ConsumeUseRefusal::ApproverNotCurrent)
    );
    assert!(
        fixture.authority.owner.lock().expect("owner").state() == &before,
        "nothing spent, no revision"
    );
    // The approver current again: the use is still there to spend.
    consume_own(&fixture, &lineage, true).expect("the use was not spent");
}

/// A store whose next append's outcome the test sets.
struct Scripted {
    inner: InMemoryGrantOwnerStore,
    next: Mutex<Option<GrantStoreError>>,
    land_before_failing: Mutex<bool>,
}
impl Scripted {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: InMemoryGrantOwnerStore::new(),
            next: Mutex::new(None),
            land_before_failing: Mutex::new(false),
        })
    }
    fn fail_next(&self, error: GrantStoreError, landed: bool) {
        *self.next.lock().expect("script") = Some(error);
        *self.land_before_failing.lock().expect("script") = landed;
    }
}
impl GrantOwnerStore for Scripted {
    fn load(&self) -> Result<Option<DurableGrantOwner>, GrantStoreError> {
        self.inner.load()
    }
    fn create(
        &self,
        guard: &AdmissionGuard,
        identity: &GrantOwnerIdentity,
    ) -> Result<(), GrantStoreError> {
        self.inner.create(guard, identity)
    }
    fn release(&self) {
        self.inner.release();
    }
    fn append(
        &self,
        guard: &AdmissionGuard,
        change: &GrantOwnerChange,
    ) -> Result<(), GrantStoreError> {
        match self.next.lock().expect("script").take() {
            None => self.inner.append(guard, change),
            Some(error) => {
                if *self.land_before_failing.lock().expect("script") {
                    self.inner.append(guard, change)?;
                }
                Err(error)
            }
        }
    }
}

#[test]
fn an_uncertain_consume_never_enters() {
    for landed in [true, false] {
        let store = Scripted::new();
        let fixture = Fixture::durable(store.clone());
        let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
        let lineage = approved_child(&fixture, &parent);
        store.fail_next(GrantStoreError::Uncertain, landed);
        assert_eq!(
            consume_own(&fixture, &lineage, true),
            Err(ConsumeUseRefusal::Uncertain),
            "an unacknowledged spend never lets the operation enter"
        );
        // The owner answers nothing until it is restored.
        assert_eq!(
            resolve_own(&fixture, &lineage, None).err(),
            Some(GrantRefusal::Unavailable)
        );
        assert_eq!(
            consume_own(&fixture, &lineage, true),
            Err(ConsumeUseRefusal::Unavailable)
        );
        drop(fixture);
        let restored = Fixture::restored(store);
        if landed {
            // The spend landed: the use stays spent.
            assert!(resolve_own(&restored, &lineage, None).is_err());
            assert_eq!(
                consume_own(&restored, &lineage, true),
                Err(ConsumeUseRefusal::Denied)
            );
        } else {
            // Nothing landed and nothing entered: the one use is still there.
            consume_own(&restored, &lineage, true).expect("spent once, after the restart");
        }
    }
}

/// The owner's retained state, as its generated machine holds it.
fn owner_state(fixture: &Fixture) -> dsl::GrantAuthorityMachineState {
    fixture
        .authority
        .owner
        .lock()
        .expect("owner")
        .state()
        .clone()
}

/// How a row ends an owner's custody of its retained facts.
#[derive(Clone, Copy, Debug)]
enum CustodyEnd {
    /// Its host's close.
    Close,
    /// An earlier append, of another grant's issuance, whose outcome is
    /// unknown; nothing of it landed.
    UncertainIssuance,
}

impl CustodyEnd {
    const ALL: [Self; 2] = [Self::Close, Self::UncertainIssuance];

    fn apply(self, fixture: &Fixture, store: &Scripted) {
        match self {
            Self::Close => fixture
                .authority
                .release_custody()
                .expect("the host's close releases custody"),
            Self::UncertainIssuance => {
                store.fail_next(GrantStoreError::Uncertain, false);
                assert_eq!(
                    fixture.authority.issue_root(
                        &who("root"),
                        id("unrelated"),
                        who("branch"),
                        None,
                        offered(),
                    ),
                    Err(GrantRefusal::Unavailable)
                );
            }
        }
    }
}

/// A spend on an owner whose custody ended, at its host's close or at an
/// earlier append whose outcome is unknown, is refused as unavailable for
/// every lineage, one the owner would deny included, before anything is
/// decided: nothing is written, the use stays unspent, and a successor
/// restored from the store spends it once.
#[test]
fn a_spend_outside_custody_is_refused_and_spends_nothing() {
    for end in CustodyEnd::ALL {
        let store = Scripted::new();
        let fixture = Fixture::durable(store.clone());
        let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
        let lineage = approved_child(&fixture, &parent);
        // The leaf without its parent: not a lineage this owner retains.
        let partial = &lineage[1..];
        assert_eq!(
            consume_own(&fixture, partial, true),
            Err(ConsumeUseRefusal::Denied),
            "in custody, a partial lineage is denied"
        );
        end.apply(&fixture, &store);
        let ended = owner_state(&fixture);
        assert_ne!(
            ended.lifecycle_phase,
            dsl::GrantAuthorityPhase::Active,
            "{end:?}"
        );
        let written = store.inner.inspect().expect("retained owner").changes.len();
        for approver_current in [true, false] {
            assert_eq!(
                consume_own(&fixture, &lineage, approver_current),
                Err(ConsumeUseRefusal::Unavailable),
                "{end:?}: the spend's own append never ran, so it is not uncertain"
            );
        }
        assert_eq!(
            consume_own(&fixture, partial, true),
            Err(ConsumeUseRefusal::Unavailable),
            "{end:?}: an owner outside custody decides nothing"
        );
        assert!(owner_state(&fixture) == ended, "{end:?}: nothing is spent");
        assert_eq!(
            store.inner.inspect().expect("retained owner").changes.len(),
            written,
            "{end:?}: nothing is written"
        );
        drop(fixture);
        let successor = Fixture::restored(store);
        consume_own(&successor, &lineage, true).expect("the use was never spent");
    }
}

/// A spend whose append outcome is unknown answers `Uncertain`, landed or
/// not: the owner records the uncertain append as the end of its custody,
/// installs nothing, invalidates every observation prepared before it and
/// refuses every later use, read and change as unavailable; its host's
/// close still hands the store back, and a successor restores what the
/// store retained. A spend the store refuses before any write is only
/// unavailable: the owner keeps its custody and spends the use next time.
#[test]
fn an_unknown_spend_outcome_is_uncertain_and_ends_the_owners_custody() {
    let store = Scripted::new();
    let fixture = Fixture::durable(store.clone());
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    let lineage = approved_child(&fixture, &parent);
    store.fail_next(GrantStoreError::Refused, false);
    assert_eq!(
        consume_own(&fixture, &lineage, true),
        Err(ConsumeUseRefusal::Unavailable),
        "a spend refused before any write is not uncertain"
    );
    assert_eq!(
        owner_state(&fixture).lifecycle_phase,
        dsl::GrantAuthorityPhase::Active
    );
    consume_own(&fixture, &lineage, true).expect("the refused spend left the use unspent");

    for landed in [true, false] {
        let store = Scripted::new();
        let fixture = Fixture::durable(store.clone());
        let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
        let lineage = approved_child(&fixture, &parent);
        let prepared = resolve_own(&fixture, &lineage, None).expect("prepared before the spend");
        let before = owner_state(&fixture);
        store.fail_next(GrantStoreError::Uncertain, landed);
        assert_eq!(
            consume_own(&fixture, &lineage, true),
            Err(ConsumeUseRefusal::Uncertain)
        );
        let after = owner_state(&fixture);
        assert_eq!(
            after.lifecycle_phase,
            dsl::GrantAuthorityPhase::Uncertain,
            "the owner records the uncertain append"
        );
        assert_eq!(
            (&after.spent, after.consumed, after.revision),
            (&before.spent, before.consumed, before.revision),
            "the spend is never installed"
        );
        assert!(
            prepared.check_current().is_err(),
            "every observation prepared before the spend is stale"
        );
        assert_eq!(
            consume_own(&fixture, &lineage, true),
            Err(ConsumeUseRefusal::Unavailable)
        );
        assert_eq!(
            resolve_own(&fixture, &lineage, None).err(),
            Some(GrantRefusal::Unavailable)
        );
        assert_eq!(
            fixture.authority.retained_grant(&lineage[1]),
            Err(GrantRefusal::Unavailable)
        );
        assert_eq!(
            fixture.authority.unrevoked_grants(|_| true),
            Err(GrantRefusal::Unavailable)
        );
        assert_eq!(
            fixture
                .authority
                .issue_root(&who("root"), id("after"), who("branch"), None, offered()),
            Err(GrantRefusal::Unavailable)
        );
        fixture
            .authority
            .release_custody()
            .expect("the host's close releases custody");
        let successor = Fixture::restored(store);
        if landed {
            assert_eq!(
                consume_own(&successor, &lineage, true),
                Err(ConsumeUseRefusal::Denied),
                "the landed spend stays spent"
            );
        } else {
            consume_own(&successor, &lineage, true)
                .expect("nothing landed: the use is still there");
        }
        drop(fixture);
    }
}

/// The retained facts of a grant are read only while the owner holds
/// custody of them: after its host's close or an append whose outcome is
/// unknown, the read is unavailable, as every lookup of such an owner is.
#[test]
fn a_retained_grant_read_outside_custody_is_unavailable() {
    for end in CustodyEnd::ALL {
        let store = Scripted::new();
        let fixture = Fixture::durable(store.clone());
        let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
        let lineage = approved_child(&fixture, &parent);
        let retained = fixture
            .authority
            .retained_grant(&lineage[1])
            .expect("read in custody");
        assert!(retained.approval.is_some());
        end.apply(&fixture, &store);
        for reference in &lineage {
            assert_eq!(
                fixture.authority.retained_grant(reference),
                Err(GrantRefusal::Unavailable),
                "{end:?}"
            );
        }
    }
}

/// The unrevoked grants are listed only while the owner holds custody of
/// its retained facts: after its host's close or an append whose outcome
/// is unknown, the listing is unavailable rather than read from memory the
/// store may no longer agree with.
#[test]
fn unrevoked_grants_outside_custody_are_unavailable() {
    for end in CustodyEnd::ALL {
        let store = Scripted::new();
        let fixture = Fixture::durable(store.clone());
        let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
        let lineage = approved_child(&fixture, &parent);
        let listed = fixture
            .authority
            .unrevoked_grants(|_| true)
            .expect("listed in custody");
        assert_eq!(listed.len(), lineage.len());
        assert!(lineage.iter().all(|reference| listed.contains(reference)));
        end.apply(&fixture, &store);
        assert_eq!(
            fixture.authority.unrevoked_grants(|_| true),
            Err(GrantRefusal::Unavailable),
            "{end:?}"
        );
    }
}

/// The record and journal shapes a reader that predates approval-carried
/// grants accepts, frozen here as that reader declares them.
mod reader_0910 {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    pub(super) struct GrantRecordV2 {
        id: EvidenceId,
        authority_incarnation: meerkat_authorization_contracts::grant::GrantAuthorityIncarnation,
        parent: Option<EvidenceId>,
        issuer: GrantPrincipal,
        grantee: GrantPrincipal,
        represented_subject: Option<GrantPrincipal>,
        issued_revision: u64,
        restrictions: ExecutionRestrictions,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "snake_case", deny_unknown_fields)]
    #[allow(dead_code)]
    pub(super) enum ChangeKindV2 {
        Issued { record: GrantRecordV2 },
        Revoked { grant_id: EvidenceId },
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    pub(super) struct ChangeV2 {
        revision: u64,
        kind: ChangeKindV2,
    }
}

#[test]
fn a_0910_reader_reads_plain_records_and_refuses_approval_records() {
    let store = Arc::new(InMemoryGrantOwnerStore::new());
    let fixture = Fixture::durable(store.clone());
    let plain = fixture.plain_parent();
    fixture
        .authority
        .issue_child(
            &AdmissionGuard::none(),
            &who("owner-agent"),
            &plain,
            id("plain-child"),
            who("branch"),
            offered(),
        )
        .expect("plain child");
    fixture
        .authority
        .revoke(
            &AdmissionGuard::none(),
            &who("root"),
            &plain,
            &mut NoControllerCustody,
        )
        .expect("revoked");
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    let lineage = approved_child(&fixture, &parent);
    consume_own(&fixture, &lineage, true).expect("spent");
    let retained = store.inspect().expect("retained owner");
    let mut kinds = Vec::new();
    for change in &retained.changes {
        let wire = serde_json::to_value(change).expect("change encodes");
        let old = serde_json::from_value::<reader_0910::ChangeV2>(wire.clone());
        let label = match &change.kind {
            durable::GrantOwnerChangeKind::Issued { record } if record.approval.is_some() => {
                "approval record"
            }
            durable::GrantOwnerChangeKind::Issued { record } if record.child_approval.is_some() => {
                "ruled record"
            }
            durable::GrantOwnerChangeKind::Issued { .. } => "plain record",
            durable::GrantOwnerChangeKind::Revoked { .. } => "revocation",
            durable::GrantOwnerChangeKind::Consumed { .. } => "consumption",
        };
        kinds.push((label, old.is_ok()));
    }
    assert_eq!(
        kinds,
        vec![
            ("plain record", true),
            ("plain record", true),
            ("revocation", true),
            ("ruled record", false),
            ("approval record", false),
            ("consumption", false),
        ],
        "plain records and revocations read as before; a rule, an approval and a spend are refused"
    );
}

#[test]
fn a_prepared_decision_before_the_spend_is_stale_after_it() {
    let fixture = Fixture::new();
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    let lineage = approved_child(&fixture, &parent);
    let prepared = resolve_own(&fixture, &lineage, None).expect("prepared before the spend");
    prepared.check_current().expect("current before the spend");
    consume_own(&fixture, &lineage, true).expect("spent");
    assert!(
        prepared.check_current().is_err(),
        "the spend published: a decision prepared before it is stale"
    );
    // Prepared afresh, the use is refused.
    assert!(resolve_own(&fixture, &lineage, None).is_err());
}

#[test]
fn resolve_refuses_another_requester_another_work_or_another_digest() {
    let fixture = Fixture::new();
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    let bound = operation("SELECT total FROM q3");
    let mut approval = fixture.approval(&parent, "decision-1");
    approval.operation = Some(bound.clone());
    approval.request_digest = fixture.terms_digest(&parent, &offered(), &approval);
    let child = fixture
        .issue(&parent, "approval-child", approval)
        .expect("bound child");
    let lineage = vec![parent, child];
    let requester = who("requester");
    let approved = work("approved-work-1");
    let resolve = |requester: &PrincipalRef,
                   work: &OriginalWorkRef,
                   operation: Option<&GrantOperationBinding>| {
        fixture.authority.resolve_lineage_with_use(
            &lineage,
            &who("branch"),
            None,
            the_use(requester, work, operation),
        )
    };
    resolve(&requester, &approved, Some(&bound)).expect("its own requester, work and operation");
    assert!(resolve(&who("someone-else"), &approved, Some(&bound)).is_err());
    assert!(resolve(&requester, &work("another-work"), Some(&bound)).is_err());
    let other = operation("SELECT * FROM salaries");
    assert!(resolve(&requester, &approved, Some(&other)).is_err());
    assert!(
        resolve(&requester, &approved, None).is_err(),
        "an operation with no digest never matches a bound grant"
    );
    // A use with no use facts at all, as a plain resolution makes it.
    assert!(
        fixture
            .authority
            .resolve_lineage(&lineage, &who("branch"), None)
            .is_err()
    );
}

#[test]
fn a_spent_use_survives_a_restart_through_the_journal() {
    let store = Arc::new(InMemoryGrantOwnerStore::new());
    let fixture = Fixture::durable(store.clone());
    let parent = fixture.ruled_parent(rule(GrantDeviceTrust::Refused));
    let lineage = approved_child(&fixture, &parent);
    consume_own(&fixture, &lineage, true).expect("spent");
    drop(fixture);
    let restored = Fixture::restored(store);
    let state = restored
        .authority
        .owner
        .lock()
        .expect("owner")
        .state()
        .clone();
    assert_eq!(state.spent.get(&lineage[1].grant_id), Some(&1));
    assert_eq!(state.consumed, 1);
    assert!(state.carried_decisions.contains(&decision("decision-1")));
    assert!(resolve_own(&restored, &lineage, None).is_err());
    assert_eq!(
        consume_own(&restored, &lineage, true),
        Err(ConsumeUseRefusal::Denied)
    );
}

/// A realm's grant log in a fresh directory, and its admission fence: the
/// store over it appends only under a guard that holds the fence.
struct FencedLog {
    _directory: tempfile::TempDir,
    fence: AdmissionFence,
    path: std::path::PathBuf,
}

impl FencedLog {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("tempdir");
        let realm = meerkat_core::connection::RealmId::parse("fenced").expect("realm id");
        let fence = AdmissionFence::realm(directory.path(), &realm).expect("fence");
        let path = fence
            .directory()
            .join("authorization")
            .join("grant-owner.jsonl");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
        Self {
            _directory: directory,
            fence,
            path,
        }
    }

    fn store(&self) -> Arc<dyn GrantOwnerStore> {
        Arc::new(FileGrantOwnerStore::new(
            &self.path,
            GrantLogFence::Realm(self.fence.clone()),
        ))
    }

    /// The log's fence, taken now without waiting.
    fn guard(&self) -> Arc<AdmissionGuard> {
        AdmissionGuard::try_acquire(vec![self.fence.clone()]).expect("the log's fence is free")
    }

    /// An admission's hold of the log's fence.
    fn hold(&self) -> meerkat_core::AdmissionHold {
        meerkat_core::AdmissionHold::try_acquire(vec![self.fence.clone()])
            .expect("an admission holds the log's fence")
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(&self.path).expect("the grant log")
    }
}

/// A root under a child rule is appended only under the grant log's
/// admission fence: while an admission holds the fence, a caller that may
/// not wait is refused at once, and a guard that does not hold the fence is
/// refused with nothing appended. Once the hold ends, the root is issued
/// under the fence and journaled.
#[test]
fn a_held_grant_log_fence_refuses_a_ruled_root_with_nothing_appended() {
    let log = FencedLog::new();
    let fixture = Fixture::fenced(&log);
    let written = log.bytes();

    let mut hold = log.hold();
    assert_eq!(
        AdmissionGuard::try_acquire(fixture.authority.admission_fences()).err(),
        Some(meerkat_core::AdmissionRefusal::InProgress),
        "a caller that may not wait is refused at once"
    );
    assert_eq!(
        fixture.try_ruled_parent(&AdmissionGuard::none(), rule(GrantDeviceTrust::Refused)),
        Err(GrantRefusal::AdmissionFence),
        "a guard that does not hold the fence is refused"
    );
    assert_eq!(log.bytes(), written, "nothing was appended");
    assert_eq!(
        fixture.authority.issued_reference(&id("owner-delegation")),
        Ok(None),
        "nothing was issued"
    );

    hold.release();
    let guard = AdmissionGuard::try_acquire(fixture.authority.admission_fences())
        .expect("the fence is free once the hold ends");
    let issued = fixture.try_ruled_parent(&guard, rule(GrantDeviceTrust::Refused));
    assert!(issued.is_ok(), "issued under the fence: {issued:?}");
    assert!(log.bytes().len() > written.len(), "and journaled");
    if let Ok(root) = issued {
        assert!(
            fixture.record(&root).child_approval.is_some(),
            "the root carries its child rule"
        );
    }
}

/// With the grant log's admission fence free, every approval-carried append
/// is made under a guard that holds it: a root and a child under a child
/// rule, the approval-carried child, and the spend of its use. A guard that
/// does not hold the fence is refused each of them first, with nothing
/// appended and the use unspent, and the journal restores exactly the
/// fenced appends.
#[test]
fn approval_carried_appends_are_made_under_the_free_grant_log_fence() {
    let log = FencedLog::new();
    let fixture = Fixture::fenced(&log);
    let guard = log.guard();
    let unfenced = AdmissionGuard::none();

    // A root under a child rule.
    assert_eq!(
        fixture.try_ruled_parent(&unfenced, rule(GrantDeviceTrust::Refused)),
        Err(GrantRefusal::AdmissionFence)
    );
    let parent = fixture
        .try_ruled_parent(&guard, rule(GrantDeviceTrust::Refused))
        .expect("the ruled root, under the fence");
    // A child under a rule of its own, of a plain parent.
    let plain = fixture
        .authority
        .issue_root(
            &guard,
            &who("root"),
            id("plain-delegation"),
            who("owner-agent"),
            None,
            delegation(1),
        )
        .expect("the plain parent, under the fence");
    let ruled_child = |guard: &AdmissionGuard| {
        fixture.authority.issue_child_with_child_rule(
            guard,
            &who("owner-agent"),
            &plain,
            id("ruled-child"),
            who("sub-agent"),
            delegation(0),
            rule(GrantDeviceTrust::Refused),
        )
    };
    assert_eq!(ruled_child(&unfenced), Err(GrantRefusal::AdmissionFence));
    ruled_child(&guard).expect("the ruled child, under the fence");
    // The approval-carried child.
    let approval = fixture.approval(&parent, "decision-1");
    let written = log.bytes();
    assert_eq!(
        fixture.issue_under(&unfenced, &parent, "approval-child", approval.clone()),
        Err(ApprovalChildRefusal::Unavailable)
    );
    assert_eq!(log.bytes(), written, "nothing was appended");
    let child = fixture
        .issue_under(&guard, &parent, "approval-child", approval)
        .expect("the approval-carried child, under the fence");
    let lineage = vec![parent, child];
    // The spend of its use.
    let written = log.bytes();
    assert_eq!(
        consume_own_under(&fixture, &unfenced, &lineage, true),
        Err(ConsumeUseRefusal::Unavailable)
    );
    assert_eq!(log.bytes(), written, "nothing was appended");
    resolve_own(&fixture, &lineage, None).expect("the refused spend spent nothing");
    consume_own_under(&fixture, &guard, &lineage, true).expect("spent under the fence");
    assert!(resolve_own(&fixture, &lineage, None).is_err());
    drop(fixture);
    drop(guard);

    // The journal holds exactly the fenced appends.
    let restored = Fixture::restored(log.store());
    let state = restored
        .authority
        .owner
        .lock()
        .expect("owner")
        .state()
        .clone();
    assert_eq!(state.records.len(), 4, "two roots and two children");
    assert_eq!(state.spent.get(&lineage[1].grant_id), Some(&1));
    assert_eq!(state.consumed, 1);
    assert!(state.carried_decisions.contains(&decision("decision-1")));
    assert!(
        restored
            .authority
            .issued_reference(&id("ruled-child"))
            .expect("the restored owner answers")
            .is_some()
    );
}
