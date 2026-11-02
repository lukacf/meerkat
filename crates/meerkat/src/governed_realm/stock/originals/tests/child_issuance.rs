//! The later effect of helper setup: the helper's child controller grant is
//! issued (or recovered) only while every originating source and the
//! independent parent grant are current for the actual setup operation.
//! The grant owner's records, revision and resolved lineages are the oracle.

use super::*;
use crate::governed_realm::helper_admission::{
    GovernedHelperAdmissionFence, HelperAdmissionRefusal,
};
use crate::governed_realm::helper_child_issuance::{
    GovernedHelperChildIssuance, HelperChildIssuanceRefusal,
};
use meerkat_authorization::grants::{ChildGrantIntent, ChildGrantOutcome};
use meerkat_authorization::work::RetainedOriginal;
use meerkat_authorization_contracts::constraints::{ExactRestriction, LifetimeRestriction};
use meerkat_authorization_contracts::grant_mutation::{
    ControllerCustodyRefusal, ControllerGrantMutationCustody,
};
use meerkat_workgraph::{
    CreateWorkItemRequest, MemoryWorkGraphStore, WorkAdmissionKey, WorkAdmissionOutcome,
    WorkGraphService, WorkNamespace,
};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

fn retained_original(name: &str) -> RetainedOriginal {
    RetainedOriginal {
        namespace: namespace(name),
        original: work(name),
    }
}

/// The native clock, set explicitly; never a sleep. The wall and monotonic
/// readings move independently; `jump_at` moves the wall clock to `jump_to`
/// on that exact read (counted from the clock's creation), so an expiry can
/// fall between a check and the final boundary read.
struct Settable {
    wall: AtomicU64,
    base: meerkat_core::time_compat::Instant,
    monotonic_ms: AtomicU64,
    reads: AtomicUsize,
    jump_at: AtomicUsize,
    jump_to: AtomicU64,
}
impl Settable {
    fn new(wall: u64) -> Self {
        Self {
            wall: AtomicU64::new(wall),
            base: meerkat_core::time_compat::Instant::now(),
            monotonic_ms: AtomicU64::new(0),
            reads: AtomicUsize::new(0),
            jump_at: AtomicUsize::new(usize::MAX),
            jump_to: AtomicU64::new(0),
        }
    }
    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
    /// Move the wall clock to `to` on the read `nth` reads from now.
    fn jump_on(&self, nth: usize, to: u64) {
        self.jump_to.store(to, Ordering::SeqCst);
        self.jump_at.store(self.reads() + nth, Ordering::SeqCst);
    }
}
impl LocalAuthorizationClock for Settable {
    fn now(&self) -> Result<LocalAuthorizationTime, meerkat_authorization::clock::LocalClockError> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        if read == self.jump_at.load(Ordering::SeqCst) {
            self.wall
                .store(self.jump_to.load(Ordering::SeqCst), Ordering::SeqCst);
        }
        Ok(LocalAuthorizationTime {
            unix_ms: self.wall.load(Ordering::SeqCst),
            monotonic: self.base
                + std::time::Duration::from_millis(self.monotonic_ms.load(Ordering::SeqCst)),
        })
    }
}

struct NoController;
impl ControllerGrantMutationCustody for NoController {
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
        Ok(mutate())
    }
}

/// One native composition: the source ledger and grant owner on one
/// publication, the setup operation prepared in the child's work context,
/// the independent parent C and an unrelated frontend controller lineage.
struct Fixture {
    ledger: Arc<StockOriginalLedger>,
    publication: LocalAuthorizationPublication,
    grants: Arc<LocalGrantAuthority>,
    clock: Arc<Settable>,
    policy: Arc<GrantBackedWorkPolicy>,
    child: InputAuthorityAssociation,
    context: meerkat_core::WorkAuthorizationContext,
    setup: PreparedOperationCheck,
    root: PrincipalRef,
    parent: GrantLineageRef,
    frontend: GrantLineageRef,
}

impl Fixture {
    fn new() -> Self {
        Self::new_with(Arc::new(Records))
    }

    /// The fixture with `operation_owner` as the records operation owner.
    fn new_with(operation_owner: Arc<dyn OperationPolicyOwner>) -> Self {
        let (ledger, publication) = ledger();
        let root = principal("grant-owner");
        let clock = Arc::new(Settable::new(100));
        let grants = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: root.clone(),
                    namespace: id("controller"),
                    generation: 1,
                },
                publication.clone(),
                Arc::clone(&clock) as Arc<dyn LocalAuthorizationClock>,
            )
            .expect("grant owner on the ledger's publication"),
        );
        let configuration = ScopedGrantComposition {
            grants: Arc::clone(&grants),
            ingress_domain: namespace("x").ingress,
            originals: Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
            invocation: ScopedInvocation::from_policy(Arc::new(Policy)),
            operation_owner,
            retained_resume: None,
        }
        .into_configuration();
        let policy = Arc::new(GrantBackedWorkPolicy::new(
            Arc::clone(&grants),
            configuration.invocation_owner,
            configuration.operation_owner,
        ));
        let controller = |name: &str| {
            grants
                .issue_root(
                    &root,
                    id(name),
                    principal("controller"),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .expect("controller grant")
        };
        let parent = controller("parent-c");
        let frontend = controller("frontend");
        ledger
            .record_candidate(&namespace("a"), &principal("requester"), original("a"))
            .expect("recorded")
            .admit()
            .expect("admitted");
        let child = association_of(&original("a"), &namespace("a"));
        let context = policy
            .work_context(Arc::from([child.clone()]), scope())
            .expect("work context");
        let setup = PreparedOperationCheck::prepare(
            context.clone(),
            PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
                operation_id: meerkat_core::OperationId::new(),
                execution_scope: context.execution_scope().clone(),
                run_id: Some(meerkat_core::RunId::new()),
                context_revision: None,
                operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                    target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                        authority: principal("owner"),
                        namespace: "records".into(),
                        id: "record".into(),
                    }),
                    usage: SourceAuthorizationUse::Read,
                }),
            }),
        )
        .expect("the setup operation is prepared");
        Self {
            ledger,
            publication,
            grants,
            clock,
            policy,
            child,
            context,
            setup,
            root,
            parent,
            frontend,
        }
    }

    /// Prepare the setup operation again, in a work context of `batch`.
    fn prepare_in(&mut self, batch: &[InputAuthorityAssociation]) {
        let context = self
            .policy
            .work_context(Arc::from(batch), scope())
            .expect("work context");
        self.setup = PreparedOperationCheck::prepare(
            context.clone(),
            PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
                operation_id: meerkat_core::OperationId::new(),
                execution_scope: context.execution_scope().clone(),
                run_id: Some(meerkat_core::RunId::new()),
                context_revision: None,
                operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                    target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                        authority: principal("owner"),
                        namespace: "records".into(),
                        id: "record".into(),
                    }),
                    usage: SourceAuthorizationUse::Read,
                }),
            }),
        )
        .expect("prepared");
    }

    /// `association` with its admitted ceiling ending at `expires_at_ms`.
    fn bounded(
        association: &InputAuthorityAssociation,
        expires_at_ms: u64,
    ) -> InputAuthorityAssociation {
        let mut claims = association.candidate().clone();
        claims.admitted_ceiling = ExecutionRestrictions {
            lifetime: LifetimeRestriction::window(0, expires_at_ms),
            ..ExecutionRestrictions::unrestricted()
        };
        InputAuthorityAssociation::new(claims).expect("well-formed claims")
    }

    /// A retained originating source `name` whose basis is a real grant
    /// with `restrictions`.
    fn source(&self, name: &str, restrictions: ExecutionRestrictions) {
        let lineage = self
            .grants
            .issue_root(
                &self.root,
                id(name),
                principal("executor"),
                None,
                restrictions,
            )
            .expect("source grant");
        let mut original = original(name);
        original.authority_basis = WorkAuthorityBasis::GrantLineage {
            lineage: vec![lineage],
        };
        self.ledger
            .record_candidate(&namespace(name), &principal("requester"), original)
            .expect("recorded")
            .admit()
            .expect("admitted");
    }

    fn intent(&self, name: &str) -> ChildGrantIntent {
        ChildGrantIntent {
            issuer: principal("controller"),
            parent_lineage: vec![self.parent.clone()],
            id: id(name),
            grantee: principal("helper"),
            represented_subject: None,
            requested: ExecutionRestrictions::unrestricted(),
        }
    }

    fn issuance(&self, name: &str, retained: &[&str]) -> GovernedHelperChildIssuance {
        GovernedHelperChildIssuance::new(
            Arc::clone(&self.grants),
            self.ledger.clone(),
            self.child.clone(),
            retained
                .iter()
                .map(|name| retained_original(name))
                .collect::<Vec<_>>()
                .into(),
            self.setup.clone(),
            Arc::clone(&self.policy),
            Arc::clone(&self.clock) as Arc<dyn LocalAuthorizationClock>,
            self.intent(name),
        )
    }

    fn issued(&self, name: &str) -> Option<GrantLineageRef> {
        self.grants.issued_reference(&id(name)).expect("lookup")
    }

    /// The publication is unchanged across `act`: no owner change landed.
    fn unchanged<T>(&self, act: impl FnOnce() -> T) -> T {
        let ((), stamp) = self.publication.observe(|| ()).expect("observed");
        let value = act();
        assert!(stamp.check_current().is_ok(), "no owner change published");
        value
    }

    fn resolves(&self, lineage: &[GrantLineageRef], executor: &str) -> bool {
        self.grants
            .resolve_controller_lineage(lineage, &principal(executor), None)
            .is_ok()
    }
}

/// Issued once with current sources, a permitted setup operation and a
/// current parent: one child record with the intended parent, issuer,
/// grantee and bounds whose controller lineage resolves for the helper.
/// The same intent recovers it without another append. After a WorkGraph
/// item is committed, withdrawing a source refuses a fresh issuance (no
/// record) and refuses usable recovery of the landed child, whose
/// historical reference stays; the committed item stays too.
#[tokio::test]
async fn a_helper_child_is_issued_only_while_its_sources_are_current() {
    let fixture = Fixture::new();
    fixture.source("p", ExecutionRestrictions::unrestricted());

    let issued = fixture
        .issuance("helper-child", &["p"])
        .issue_or_recover()
        .expect("issued");
    let ChildGrantOutcome::Issued { lineage } = issued.clone() else {
        panic!("first attempt issues: {issued:?}")
    };
    assert_eq!(&lineage[..1], std::slice::from_ref(&fixture.parent));
    assert_eq!(fixture.issued("helper-child").as_ref(), lineage.last());
    assert!(fixture.resolves(&lineage, "helper"), "the helper's lineage");
    assert!(
        !fixture.resolves(&lineage, "controller"),
        "the child is the helper's, not its issuer's"
    );
    let recovered = fixture.unchanged(|| {
        fixture
            .issuance("helper-child", &["p"])
            .issue_or_recover()
            .expect("recovered")
    });
    assert_eq!(
        recovered,
        ChildGrantOutcome::Recovered {
            lineage: lineage.clone()
        }
    );

    // A WorkGraph item is committed under the fence first.
    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let request = || CreateWorkItemRequest {
        title: "setup helper".to_string(),
        ..Default::default()
    };
    let key = WorkAdmissionKey::new("helper-2").expect("key");
    let fence = GovernedHelperAdmissionFence::new(
        Arc::clone(&fixture.grants),
        fixture.ledger.clone(),
        fixture.child.clone(),
        Arc::from([retained_original("p")]),
        fixture.setup.clone(),
        Arc::clone(&fixture.policy),
        Arc::clone(&fixture.clock) as Arc<dyn LocalAuthorizationClock>,
        "default",
        WorkNamespace::default(),
        service
            .admission_identity(key.clone(), request())
            .expect("identity"),
    );
    assert!(matches!(
        service
            .create_idempotent_with_fence(key, request(), fence)
            .await,
        Ok(WorkAdmissionOutcome::Created(_))
    ));
    fixture
        .ledger
        .cancel_operations(&namespace("p"), &work("p"), evidence("withdrawn"))
        .expect("the source is withdrawn");

    let refused = fixture.unchanged(|| {
        fixture
            .issuance("helper-child-2", &["p"])
            .issue_or_recover()
    });
    assert_eq!(refused, Err(HelperChildIssuanceRefusal::Denied));
    assert_eq!(fixture.issued("helper-child-2"), None, "nothing appended");
    let historical =
        fixture.unchanged(|| fixture.issuance("helper-child", &["p"]).issue_or_recover());
    assert_eq!(
        historical,
        Err(HelperChildIssuanceRefusal::Denied),
        "a landed child is not usable once its source is withdrawn"
    );
    assert_eq!(
        fixture.issued("helper-child").as_ref(),
        lineage.last(),
        "its historical reference is kept, not deleted or reissued"
    );
    assert!(fixture.resolves(std::slice::from_ref(&fixture.parent), "controller"));
    assert_eq!(
        service
            .list(meerkat_workgraph::WorkItemFilter {
                include_terminal: true,
                ..Default::default()
            })
            .await
            .expect("list")
            .len(),
        1,
        "the committed WorkGraph item stays"
    );
}

/// Every originating source stays a conjunct: a source whose grant excludes
/// the operation refuses although the other allows it; a source whose
/// finite grant has expired on the native clock refuses although it allowed
/// the operation before. The independent parent is current or nothing is
/// issued: once it is revoked, no other controller lineage stands in.
#[tokio::test]
async fn every_originating_source_and_the_parent_are_conjuncts() {
    let fixture = Fixture::new();
    fixture.source("p", ExecutionRestrictions::unrestricted());
    fixture.source(
        "r",
        ExecutionRestrictions {
            actions: ExactRestriction::exact([ActionRef {
                feature: "stock-test".into(),
                action: "write".into(),
            }]),
            ..ExecutionRestrictions::unrestricted()
        },
    );
    fixture.source(
        "b",
        ExecutionRestrictions {
            lifetime: LifetimeRestriction::window(0, 500),
            ..ExecutionRestrictions::unrestricted()
        },
    );

    let restricted = fixture.unchanged(|| {
        fixture
            .issuance("with-restrictive", &["p", "r"])
            .issue_or_recover()
    });
    assert_eq!(restricted, Err(HelperChildIssuanceRefusal::Denied));
    assert_eq!(fixture.issued("with-restrictive"), None);
    assert!(
        matches!(
            fixture.issuance("anchor-only", &["p"]).issue_or_recover(),
            Ok(ChildGrantOutcome::Issued { .. })
        ),
        "the permissive source alone permits it"
    );

    assert!(matches!(
        fixture
            .issuance("before-expiry", &["p", "b"])
            .issue_or_recover(),
        Ok(ChildGrantOutcome::Issued { .. })
    ));
    fixture.clock.wall.store(500, Ordering::SeqCst);
    let expired = fixture.unchanged(|| {
        fixture
            .issuance("after-expiry", &["p", "b"])
            .issue_or_recover()
    });
    assert_eq!(expired, Err(HelperChildIssuanceRefusal::Denied));
    assert_eq!(fixture.issued("after-expiry"), None);
    assert!(fixture.resolves(std::slice::from_ref(&fixture.parent), "controller"));

    fixture
        .grants
        .revoke(&fixture.root, &fixture.parent, &mut NoController)
        .expect("the independent parent is revoked");
    let orphaned =
        fixture.unchanged(|| fixture.issuance("after-revoke", &["p"]).issue_or_recover());
    assert_eq!(orphaned, Err(HelperChildIssuanceRefusal::Denied));
    assert_eq!(fixture.issued("after-revoke"), None);
    assert!(
        fixture.resolves(std::slice::from_ref(&fixture.frontend), "controller"),
        "the frontend controller lineage is current and is not a fallback"
    );
}

/// The window carried to the final append and recovery checks is the
/// exact native decision's: a batch member's admitted ceiling bounds it
/// although the source, the policy owner and every grant are unbounded; an
/// expiry that falls after the check, on the final boundary read, refuses
/// both a fresh issuance and a usable recovery; and the decision's own
/// monotonic cutoff is kept, not rebuilt from a later read.
#[tokio::test]
async fn the_exact_decision_window_bounds_the_final_boundary() {
    let mut fixture = Fixture::new();
    fixture.source("p", ExecutionRestrictions::unrestricted());
    // Native batch A+B: B is admitted with a ceiling ending at 500.
    ledger_admit(&fixture, "b");
    let b = Fixture::bounded(&association_of(&original("b"), &namespace("b")), 500);
    let a = fixture.child.clone();
    fixture.prepare_in(&[a, b]);

    let validity = fixture
        .issuance("probe", &["p"])
        .originating()
        .expect("current");
    assert_eq!(
        (validity.not_before_ms, validity.expires_at_ms),
        (0, 500),
        "B's admitted ceiling, although A, the policy and the grants are unbounded"
    );
    assert_eq!(
        validity.deadline,
        fixture.clock.base + std::time::Duration::from_millis(400),
        "the decision's own monotonic cutoff"
    );

    // A wall rollback with monotonic time passing: the cutoff stays the
    // decision's (base + 400), not a later read's.
    fixture.clock.monotonic_ms.store(399, Ordering::SeqCst);
    let later = fixture
        .issuance("probe", &["p"])
        .originating()
        .expect("still current");
    assert_eq!(later.deadline, validity.deadline);

    // Count the reads of a fresh issuance and of a recovery in the state the
    // later attempts run in (each issuance publishes, so every later attempt
    // re-prepares the setup operation): the last read is the final boundary.
    assert!(matches!(
        fixture.issuance("issued", &["p"]).issue_or_recover(),
        Ok(ChildGrantOutcome::Issued { .. })
    ));
    let before = fixture.clock.reads();
    assert!(matches!(
        fixture.issuance("issued-again", &["p"]).issue_or_recover(),
        Ok(ChildGrantOutcome::Issued { .. })
    ));
    let issue_reads = fixture.clock.reads() - before;
    let before = fixture.clock.reads();
    assert!(matches!(
        fixture.issuance("issued", &["p"]).issue_or_recover(),
        Ok(ChildGrantOutcome::Recovered { .. })
    ));
    let recovery_reads = fixture.clock.reads() - before;

    // Expiry on the final boundary read of a fresh issuance.
    let before = fixture.clock.reads();
    fixture.clock.jump_on(issue_reads, 500);
    let late = fixture.unchanged(|| fixture.issuance("late", &["p"]).issue_or_recover());
    assert_eq!(late, Err(HelperChildIssuanceRefusal::Denied));
    assert_eq!(
        fixture.clock.reads() - before,
        issue_reads,
        "refused at the final boundary read, after every earlier check passed"
    );
    assert_eq!(fixture.issued("late"), None, "nothing appended");

    // Expiry on the final boundary read of a recovery.
    fixture.clock.wall.store(100, Ordering::SeqCst);
    let before = fixture.clock.reads();
    fixture.clock.jump_on(recovery_reads, 500);
    assert_eq!(
        fixture.issuance("issued", &["p"]).issue_or_recover(),
        Err(HelperChildIssuanceRefusal::Denied),
        "a landed child is not recovered as usable past the window"
    );
    assert_eq!(fixture.clock.reads() - before, recovery_reads);
    assert!(fixture.issued("issued").is_some(), "its record is kept");
}

/// A further originating grant's own lower bound stays in the window: with
/// the prepared decision usable from 0 but a retained original's grant only
/// from 100, a final boundary read at 50 refuses a fresh issuance (no
/// record) and a usable recovery (record kept).
#[tokio::test]
async fn a_further_grants_not_before_bounds_the_final_boundary() {
    let fixture = Fixture::new();
    fixture.source(
        "w",
        ExecutionRestrictions {
            lifetime: LifetimeRestriction::window(100, 1000),
            ..ExecutionRestrictions::unrestricted()
        },
    );
    fixture.clock.wall.store(200, Ordering::SeqCst);
    let validity = fixture
        .issuance("probe", &["w"])
        .originating()
        .expect("current at 200");
    assert_eq!(
        (validity.not_before_ms, validity.expires_at_ms),
        (100, 1000)
    );

    assert!(matches!(
        fixture.issuance("issued", &["w"]).issue_or_recover(),
        Ok(ChildGrantOutcome::Issued { .. })
    ));
    let before = fixture.clock.reads();
    assert!(matches!(
        fixture.issuance("issued-again", &["w"]).issue_or_recover(),
        Ok(ChildGrantOutcome::Issued { .. })
    ));
    let issue_reads = fixture.clock.reads() - before;
    let before = fixture.clock.reads();
    assert!(matches!(
        fixture.issuance("issued", &["w"]).issue_or_recover(),
        Ok(ChildGrantOutcome::Recovered { .. })
    ));
    let recovery_reads = fixture.clock.reads() - before;

    let before = fixture.clock.reads();
    fixture.clock.jump_on(issue_reads, 50);
    let early = fixture.unchanged(|| fixture.issuance("early", &["w"]).issue_or_recover());
    assert_eq!(early, Err(HelperChildIssuanceRefusal::Denied));
    assert_eq!(fixture.clock.reads() - before, issue_reads);
    assert_eq!(fixture.issued("early"), None, "nothing appended");

    fixture.clock.wall.store(200, Ordering::SeqCst);
    let before = fixture.clock.reads();
    fixture.clock.jump_on(recovery_reads, 50);
    assert_eq!(
        fixture.issuance("issued", &["w"]).issue_or_recover(),
        Err(HelperChildIssuanceRefusal::Denied)
    );
    assert_eq!(fixture.clock.reads() - before, recovery_reads);
    assert!(fixture.issued("issued").is_some(), "its record is kept");
}

fn ledger_admit(fixture: &Fixture, name: &str) {
    fixture
        .ledger
        .record_candidate(&namespace(name), &principal("requester"), original(name))
        .expect("recorded")
        .admit()
        .expect("admitted");
}

/// The dispatch path's work owner issues the child through its own single
/// reservation: issued once, recovered on retry, refused for a mistyped
/// intent (nothing issued) and for a withdrawn source.
#[test]
fn the_work_owner_issues_the_child_under_its_own_reservation() {
    use meerkat_authorization::grants::ChildGrantRefusal;
    use meerkat_authorization::work::FurtherOriginals;
    use meerkat_core::authorization::OperationReviewTier;
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;

    let f = Fixture::new();
    f.source("s", ExecutionRestrictions::unrestricted());
    let further = FurtherOriginals {
        originals: Arc::from([retained_original("s")]),
        source: Arc::new(SourceOriginalBasis::new(f.ledger.clone())),
    };
    let issue = |intent: &(dyn std::any::Any + Send + Sync), tier| {
        f.context.authorization().issue_child_if_current_within(
            f.setup.binding(),
            f.setup.validity().as_ref(),
            &further,
            intent,
            tier,
        )
    };
    let outcome = |boxed: Box<dyn std::any::Any + Send>| {
        *boxed
            .downcast::<Result<ChildGrantOutcome, ChildGrantRefusal>>()
            .expect("the grant owner's typed outcome")
    };

    assert!(
        matches!(
            issue(&"not an intent", OperationReviewTier::R1),
            Err(meerkat_core::OperationAuthorizationError::Refused(_))
        ),
        "a mistyped intent is refused before any effect"
    );
    assert!(f.issued("helper-child").is_none());

    let issued =
        outcome(issue(&f.intent("helper-child"), OperationReviewTier::R1).expect("issued"));
    assert!(matches!(issued, Ok(ChildGrantOutcome::Issued { .. })));
    assert!(f.issued("helper-child").is_some());
    let recovered = f.unchanged(|| {
        outcome(issue(&f.intent("helper-child"), OperationReviewTier::R1).expect("recovered"))
    });
    assert!(matches!(recovered, Ok(ChildGrantOutcome::Recovered { .. })));

    f.ledger
        .cancel_operations(&namespace("s"), &work("s"), evidence("cancel"))
        .expect("the source is withdrawn");
    assert!(
        matches!(
            issue(&f.intent("helper-child-2"), OperationReviewTier::R1),
            Err(meerkat_core::OperationAuthorizationError::Refused(_))
        ),
        "a withdrawn source refuses before any append"
    );
    assert!(f.issued("helper-child-2").is_none());
}

/// A default dispatch context has no governed setup operation: no child
/// issuance can be built from it.
#[test]
fn a_default_dispatch_context_builds_no_child_issuance() {
    let fixture = Fixture::new();
    let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("args");
    assert_eq!(
        GovernedHelperChildIssuance::from_dispatch(
            meerkat_core::ToolCallView {
                id: "setup",
                name: "setup",
                args: &args,
            },
            &meerkat_core::ToolDispatchContext::default(),
            fixture.ledger.clone(),
            Arc::from([]),
            fixture.intent("from-default"),
        )
        .err(),
        Some(HelperChildIssuanceRefusal::Denied)
    );
    assert_eq!(fixture.issued("from-default"), None);
}

/// The records owner whose review tier the host can raise after preparation.
struct TieredRecords(std::sync::atomic::AtomicBool);
impl OperationPolicyOwner for TieredRecords {
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let mut allowance = Records.authorize_operation(association, binding, purpose, now_ms)?;
        if self.0.load(Ordering::SeqCst) {
            allowance.review_tier = meerkat_core::authorization::OperationReviewTier::R2;
        }
        Ok(allowance)
    }
}

/// A current decision that now needs R2 while the dispatch entered only R1
/// refuses the issuance, and nothing is issued.
#[test]
fn a_decision_needing_more_review_than_entered_issues_nothing() {
    use meerkat_authorization::work::FurtherOriginals;
    use meerkat_core::authorization::OperationReviewTier;
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;

    let tier = Arc::new(TieredRecords(std::sync::atomic::AtomicBool::new(false)));
    let f = Fixture::new_with(tier.clone());
    f.source("s", ExecutionRestrictions::unrestricted());
    tier.0.store(true, Ordering::SeqCst);
    let further = FurtherOriginals {
        originals: Arc::from([retained_original("s")]),
        source: Arc::new(SourceOriginalBasis::new(f.ledger.clone())),
    };
    let result = f.unchanged(|| {
        f.context.authorization().issue_child_if_current_within(
            f.setup.binding(),
            f.setup.validity().as_ref(),
            &further,
            &f.intent("over-tier"),
            OperationReviewTier::R1,
        )
    });
    assert!(
        matches!(
            result,
            Err(meerkat_core::OperationAuthorizationError::Refused(_))
        ),
        "a decision needing R2 refuses an R1 entry"
    );
    assert!(f.issued("over-tier").is_none(), "nothing was issued");
}

/// Every lower bound holds: a further original's grant valid over
/// [200, 500) admits at 250, and a wall rollback to 100 refuses although the
/// held decision itself started at 0.
#[test]
fn a_further_grant_lower_bound_survives_a_wall_rollback() {
    use meerkat_authorization::work::FurtherOriginals;
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;

    let f = Fixture::new();
    f.source(
        "g",
        ExecutionRestrictions {
            lifetime: LifetimeRestriction::window(200, 500),
            ..ExecutionRestrictions::unrestricted()
        },
    );
    let further = FurtherOriginals {
        originals: Arc::from([retained_original("g")]),
        source: Arc::new(SourceOriginalBasis::new(f.ledger.clone())),
    };
    let run = || {
        let mut ran = false;
        let result = f.context.authorization().run_if_current_within(
            f.setup.binding(),
            f.setup.validity().as_ref(),
            &further,
            &mut || ran = true,
        );
        (result, ran)
    };
    f.clock.wall.store(250, Ordering::SeqCst);
    assert_eq!(run(), (Ok(()), true), "inside the grant's window");
    f.clock.wall.store(100, Ordering::SeqCst);
    let (result, ran) = run();
    assert!(
        matches!(
            result,
            Err(meerkat_core::OperationAuthorizationError::Refused(_))
        ) && !ran,
        "before the grant's lower bound after a rollback"
    );
}

/// RED-3 (R4): a further original that is still its owner's pending
/// candidate leaves the child unissued and the issuance unavailable
/// (retried), never refused; once the owner promotes it, the child is
/// issued. CONTROL-cancelled (R4): cancelled while pending, it is refused.
#[test]
fn a_pending_further_original_leaves_the_child_unissued_and_unavailable() {
    use meerkat_authorization::work::FurtherOriginals;
    use meerkat_core::authorization::OperationReviewTier;
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;

    let f = Fixture::new();
    let issue = |source: &str, child: &str| {
        let further = FurtherOriginals {
            originals: Arc::from([retained_original(source)]),
            source: Arc::new(SourceOriginalBasis::new(f.ledger.clone())),
        };
        f.context.authorization().issue_child_if_current_within(
            f.setup.binding(),
            f.setup.validity().as_ref(),
            &further,
            &f.intent(child),
            OperationReviewTier::R1,
        )
    };
    let pending_s = f
        .ledger
        .record_candidate(&namespace("s"), &principal("requester"), original("s"))
        .expect("recorded");
    assert!(
        matches!(
            issue("s", "while-pending"),
            Err(meerkat_core::OperationAuthorizationError::Unavailable)
        ),
        "a pending further original is not yet, never refused"
    );
    assert!(f.issued("while-pending").is_none(), "nothing was issued");
    pending_s.admit().expect("promoted");
    assert!(issue("s", "once-promoted").is_ok(), "issued once promoted");
    assert!(f.issued("once-promoted").is_some());

    let _pending_t = f
        .ledger
        .record_candidate(&namespace("t"), &principal("requester"), original("t"))
        .expect("recorded");
    f.ledger
        .cancel_operations(&namespace("t"), &work("t"), evidence("cancel-t"))
        .expect("a pending candidate can be cancelled");
    assert!(
        matches!(
            issue("t", "cancelled"),
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::Denied
        ),
        "a cancellation wins over a pending admission"
    );
    assert!(f.issued("cancelled").is_none());
}

/// The records owner, refusing as the host sets it after preparation: 1
/// refuses to rebuild (`ReprepareRequired`), 2 denies, 3 refuses malformed
/// facts, 0 is [`Records`].
struct ChurnedRecords(std::sync::atomic::AtomicU8);
impl OperationPolicyOwner for ChurnedRecords {
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        use meerkat_core::authorization::{OperationRefusalKind, OperationRefused};
        match self.0.load(Ordering::SeqCst) {
            1 => Err(OperationRefused::new(OperationRefusalKind::ReprepareRequired).into()),
            2 => Err(OperationRefused::new(OperationRefusalKind::Denied).into()),
            3 => Err(OperationRefused::new(OperationRefusalKind::MalformedFacts).into()),
            _ => Records.authorize_operation(association, binding, purpose, now_ms),
        }
    }
}

/// Each refusal mode of [`ChurnedRecords`] with what the fence and the child
/// issuance must answer for it: only a refusal to rebuild is unavailable.
const LEAF_REFUSALS: [(u8, HelperAdmissionRefusal, HelperChildIssuanceRefusal); 3] = [
    (
        1,
        HelperAdmissionRefusal::Unavailable,
        HelperChildIssuanceRefusal::Unavailable,
    ),
    (
        2,
        HelperAdmissionRefusal::Denied,
        HelperChildIssuanceRefusal::Denied,
    ),
    (
        3,
        HelperAdmissionRefusal::Denied,
        HelperChildIssuanceRefusal::Denied,
    ),
];

fn setup_request() -> CreateWorkItemRequest {
    CreateWorkItemRequest {
        title: "setup helper".to_string(),
        ..Default::default()
    }
}

/// A refusal to rebuild at the prepared effect leaf is no denial: the
/// prepared fence commits nothing and the prepared child issuance appends
/// nothing, both unavailable, and each succeeds once the owners answer
/// again. A denial and malformed facts stay denials on both.
#[tokio::test]
async fn a_rebuild_refusal_at_the_prepared_effect_leaf_is_unavailable() {
    let owner = Arc::new(ChurnedRecords(std::sync::atomic::AtomicU8::new(0)));
    let fixture = Fixture::new_with(owner.clone());
    fixture.source("p", ExecutionRestrictions::unrestricted());
    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let fence = |key: &str| {
        let key = WorkAdmissionKey::new(key).expect("key");
        let fence = GovernedHelperAdmissionFence::new(
            Arc::clone(&fixture.grants),
            fixture.ledger.clone(),
            fixture.child.clone(),
            Arc::from([retained_original("p")]),
            fixture.setup.clone(),
            Arc::clone(&fixture.policy),
            Arc::clone(&fixture.clock) as Arc<dyn LocalAuthorizationClock>,
            "default",
            WorkNamespace::default(),
            service
                .admission_identity(key.clone(), setup_request())
                .expect("identity"),
        );
        (key, fence)
    };

    for (mode, admission, issuance) in LEAF_REFUSALS {
        owner.0.store(mode, Ordering::SeqCst);
        let (key, refused) = fence(&format!("helper-{mode}"));
        assert!(
            service
                .create_idempotent_with_fence(key, setup_request(), refused.clone())
                .await
                .is_err()
        );
        assert_eq!(refused.refusal(), Some(admission), "fence, mode {mode}");
        assert_eq!(
            fixture.issuance("helper-child", &["p"]).issue_or_recover(),
            Err(issuance),
            "issuance, mode {mode}"
        );
        assert_eq!(fixture.issued("helper-child"), None, "nothing appended");
    }
    assert!(
        service
            .list(meerkat_workgraph::WorkItemFilter {
                include_terminal: true,
                ..Default::default()
            })
            .await
            .expect("list")
            .is_empty(),
        "nothing committed"
    );

    owner.0.store(0, Ordering::SeqCst);
    let (key, admitted) = fence("helper-current");
    assert!(matches!(
        service
            .create_idempotent_with_fence(key, setup_request(), admitted)
            .await,
        Ok(WorkAdmissionOutcome::Created(_))
    ));
    assert!(matches!(
        fixture.issuance("helper-child", &["p"]).issue_or_recover(),
        Ok(ChildGrantOutcome::Issued { .. })
    ));
}

/// The dispatch paths answer the same when the dispatch's own work owners
/// refuse under their exclusion: unavailable for a refusal to rebuild,
/// denied for a denial or malformed facts, at the fence and at the child
/// issuance; both succeed once the owners answer again.
#[tokio::test]
async fn a_rebuild_refusal_at_the_dispatch_effect_leaf_is_unavailable() {
    use meerkat_core::authorization::{ToolAuthorizationFacts, ToolAuthorizationTarget};

    let owner = Arc::new(ChurnedRecords(std::sync::atomic::AtomicU8::new(0)));
    let fixture = Fixture::new_with(owner.clone());
    fixture.source("s", ExecutionRestrictions::unrestricted());
    // The dispatch's own prepared tool operation in the child's work.
    let plan = meerkat_core::ToolExecutionContract::default()
        .resolve_default(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(1),
                ),
            ])
            .expect("deadlines"),
        )
        .expect("plan");
    let arguments: Arc<serde_json::value::RawValue> =
        Arc::from(serde_json::value::RawValue::from_string("{}".to_string()).expect("arguments"));
    let prepared = PreparedOperationCheck::prepare(
        fixture.context.clone(),
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId::new(),
            execution_scope: fixture.context.execution_scope().clone(),
            run_id: None,
            context_revision: None,
            operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
                call_id: Arc::from("setup-call"),
                name: "setup".into(),
                arguments,
                target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
            }),
        }),
    )
    .expect("the dispatch's operation is prepared");
    let context = meerkat_core::ToolDispatchContext::default()
        .with_work_authorization(Some(fixture.context.clone()))
        .__test_with_prepared_authorization(prepared.clone());
    // The dispatch's own call, borrowed from the binding it was prepared with.
    let call = match &prepared.binding().facts().operation {
        AuthorizationOperation::Tool(tool) => Some(tool.call()),
        _ => None,
    }
    .expect("a tool operation");
    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let fence = |key: &str| {
        let key = WorkAdmissionKey::new(key).expect("key");
        let fence = GovernedHelperAdmissionFence::from_dispatch(
            call,
            &context,
            fixture.ledger.clone(),
            Arc::from([retained_original("s")]),
            "default",
            WorkNamespace::default(),
            service
                .admission_identity(key.clone(), setup_request())
                .expect("identity"),
        )
        .expect("the dispatch's own operation builds the fence");
        (key, fence)
    };
    let issuance = || {
        GovernedHelperChildIssuance::from_dispatch(
            call,
            &context,
            fixture.ledger.clone(),
            Arc::from([retained_original("s")]),
            fixture.intent("dispatch-child"),
        )
        .expect("the dispatch's own operation builds the issuance")
    };

    for (mode, admission, refused_issuance) in LEAF_REFUSALS {
        owner.0.store(mode, Ordering::SeqCst);
        let (key, refused) = fence(&format!("helper-{mode}"));
        assert!(
            service
                .create_idempotent_with_fence(key, setup_request(), refused.clone())
                .await
                .is_err()
        );
        assert_eq!(refused.refusal(), Some(admission), "fence, mode {mode}");
        assert_eq!(
            issuance().issue_or_recover(),
            Err(refused_issuance),
            "issuance, mode {mode}"
        );
        assert_eq!(fixture.issued("dispatch-child"), None, "nothing appended");
    }

    owner.0.store(0, Ordering::SeqCst);
    let (key, admitted) = fence("helper-current");
    assert!(matches!(
        service
            .create_idempotent_with_fence(key, setup_request(), admitted)
            .await,
        Ok(WorkAdmissionOutcome::Created(_))
    ));
    assert!(matches!(
        issuance().issue_or_recover(),
        Ok(ChildGrantOutcome::Issued { .. })
    ));
}

// ---------------------------------------------------------------------------
// Entered governed operations: further steps of a dispatch's entered
// operation (`EnteredDispatchOperation`) through the native owners, and the
// dispatch-path classification of review settlements.
// ---------------------------------------------------------------------------

/// The records owner at a review tier the host sets after preparation: 0 is
/// [`Records`] (R1), 1 raises it to R2 and 2 to R3.
struct TierSet(std::sync::atomic::AtomicU8);
impl TierSet {
    fn at(tier: u8) -> Arc<Self> {
        Arc::new(Self(std::sync::atomic::AtomicU8::new(tier)))
    }
    fn set(&self, tier: u8) {
        self.0.store(tier, Ordering::SeqCst);
    }
}
impl OperationPolicyOwner for TierSet {
    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        use meerkat_core::authorization::OperationReviewTier;
        let mut allowance = Records.authorize_operation(association, binding, purpose, now_ms)?;
        allowance.review_tier = match self.0.load(Ordering::SeqCst) {
            0 => OperationReviewTier::R1,
            1 => OperationReviewTier::R2,
            _ => OperationReviewTier::R3,
        };
        Ok(allowance)
    }
}

fn retained_originals(names: &[&str]) -> Arc<[RetainedOriginal]> {
    names
        .iter()
        .map(|name| retained_original(name))
        .collect::<Vec<_>>()
        .into()
}

/// A governed tool dispatch in `work`: its own prepared tool operation and
/// the dispatch context carrying it, as the fenced dispatch attaches them.
/// No review composition is attached outside a fenced dispatch, so the
/// context carries no review entry: an entry it makes is unreviewed (R1),
/// and an R2 or R3 decision cannot enter here.
struct Dispatch {
    prepared: PreparedOperationCheck,
    context: meerkat_core::ToolDispatchContext,
}

impl Dispatch {
    fn new(work: &meerkat_core::WorkAuthorizationContext) -> Self {
        use meerkat_core::authorization::{ToolAuthorizationFacts, ToolAuthorizationTarget};
        let plan = meerkat_core::ToolExecutionContract::default()
            .resolve_default(
                meerkat_core::ToolDeadlineChain::new(vec![
                    meerkat_core::ToolDeadlineContributor::finite(
                        meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                        std::time::Duration::from_secs(1),
                    ),
                ])
                .expect("deadlines"),
            )
            .expect("plan");
        let arguments: Arc<serde_json::value::RawValue> = Arc::from(
            serde_json::value::RawValue::from_string("{}".to_string()).expect("arguments"),
        );
        let prepared = PreparedOperationCheck::prepare(
            work.clone(),
            PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
                operation_id: meerkat_core::OperationId::new(),
                execution_scope: work.execution_scope().clone(),
                run_id: None,
                context_revision: None,
                operation: AuthorizationOperation::Tool(ToolAuthorizationFacts {
                    call_id: Arc::from("setup-call"),
                    name: "setup".into(),
                    arguments,
                    target: ToolAuthorizationTarget::Dispatcher(Arc::new(plan)),
                }),
            }),
        )
        .expect("the dispatch's operation is prepared");
        let context = meerkat_core::ToolDispatchContext::default()
            .with_work_authorization(Some(work.clone()))
            .__test_with_prepared_authorization(prepared.clone());
        Self { prepared, context }
    }

    /// The dispatch's own call, borrowed from the binding it was prepared
    /// with.
    fn call(&self) -> meerkat_core::ToolCallView<'_> {
        match &self.prepared.binding().facts().operation {
            AuthorizationOperation::Tool(tool) => Some(tool.call()),
            _ => None,
        }
        .expect("a tool operation")
    }

    /// The dispatch's single consuming entry, made while it is current.
    fn enter(&self) -> meerkat_core::EnteredDispatchOperation {
        self.context
            .enter_governed_operation(self.call())
            .expect("the unreviewed dispatch enters")
    }
}

fn entered_issuance(
    f: &Fixture,
    entered: &meerkat_core::EnteredDispatchOperation,
    retained: &[&str],
    name: &str,
) -> GovernedHelperChildIssuance {
    GovernedHelperChildIssuance::from_entered(
        entered,
        f.ledger.clone(),
        retained_originals(retained),
        f.intent(name),
    )
}

/// An entered step's core answer is an operation denial with no
/// not-dispatched certificate (the call's effect may already have begun),
/// never a review settlement.
fn assert_step_denial(result: Result<(), meerkat_core::ToolError>, label: &str) {
    match result {
        Err(meerkat_core::ToolError::AuthorizationRefused { refusal, .. }) => {
            assert_eq!(
                refusal.kind(),
                meerkat_core::OperationRefusalKind::Denied,
                "{label}: a denial"
            );
            assert_eq!(
                refusal.dispatch(),
                meerkat_core::RefusalDispatch::Unknown,
                "{label}: no not-dispatched certificate once the call entered"
            );
        }
        other => panic!("{label}: an entered step answers an operation denial, not {other:?}"),
    }
}

/// Every step of `entered` is refused for that step only: the fence commits
/// nothing and the child issuance appends nothing, both `Denied`, and the
/// core step answers an operation denial (see [`assert_step_denial`]) for a
/// run, which never runs its operation, and for an issuance.
async fn assert_entered_steps_denied(
    f: &Fixture,
    entered: &meerkat_core::EnteredDispatchOperation,
    retained: &[&str],
    label: &str,
) {
    use meerkat_authorization::work::FurtherOriginals;
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;

    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let key = WorkAdmissionKey::new(format!("helper-{label}")).expect("key");
    let fence = GovernedHelperAdmissionFence::from_entered(
        entered,
        f.ledger.clone(),
        retained_originals(retained),
        "default",
        WorkNamespace::default(),
        service
            .admission_identity(key.clone(), setup_request())
            .expect("identity"),
    );
    assert!(
        service
            .create_idempotent_with_fence(key, setup_request(), fence.clone())
            .await
            .is_err(),
        "{label}: the fence refuses"
    );
    assert_eq!(
        fence.refusal(),
        Some(HelperAdmissionRefusal::Denied),
        "{label}: fence"
    );
    assert!(
        service
            .list(meerkat_workgraph::WorkItemFilter {
                include_terminal: true,
                ..Default::default()
            })
            .await
            .expect("list")
            .is_empty(),
        "{label}: nothing committed"
    );

    let child = format!("child-{label}");
    assert_eq!(
        entered_issuance(f, entered, retained, &child).issue_or_recover(),
        Err(HelperChildIssuanceRefusal::Denied),
        "{label}: issuance"
    );
    assert_eq!(f.issued(&child), None, "{label}: nothing appended");

    let further = FurtherOriginals {
        originals: retained_originals(retained),
        source: Arc::new(SourceOriginalBasis::new(f.ledger.clone())),
    };
    let mut ran = false;
    let run = entered.run_if_current_within(&further, &mut || ran = true);
    assert_step_denial(run, label);
    assert!(!ran, "{label}: the step's operation never ran");
    assert_step_denial(
        entered
            .issue_child_if_current_within(&further, &f.intent(&child))
            .map(drop),
        label,
    );
    assert_eq!(
        f.issued(&child),
        None,
        "{label}: nothing appended by the step"
    );
}

/// One entered witness serves the helper fence and every child issuance of
/// its call: the fence commits the helper item, two intents are issued, and
/// a retried intent is recovered, each decided afresh by the dispatch's own
/// work owners. An unreviewed dispatch enters at R1 and spends nothing, so
/// it can enter again.
#[tokio::test]
async fn one_entered_witness_serves_the_fence_and_every_child_issuance() {
    use meerkat_core::authorization::OperationReviewTier;

    let f = Fixture::new();
    f.source("s", ExecutionRestrictions::unrestricted());
    let dispatch = Dispatch::new(&f.context);
    let entered = dispatch.enter();
    assert_eq!(entered.entered_tier(), OperationReviewTier::R1);

    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let key = WorkAdmissionKey::new("helper-entered").expect("key");
    let fence = GovernedHelperAdmissionFence::from_entered(
        &entered,
        f.ledger.clone(),
        retained_originals(&["s"]),
        "default",
        WorkNamespace::default(),
        service
            .admission_identity(key.clone(), setup_request())
            .expect("identity"),
    );
    assert!(matches!(
        service
            .create_idempotent_with_fence(key, setup_request(), fence.clone())
            .await,
        Ok(WorkAdmissionOutcome::Created(_))
    ));
    assert_eq!(fence.refusal(), None, "the step was admitted");

    let first = entered_issuance(&f, &entered, &["s"], "c1")
        .issue_or_recover()
        .expect("c1 issued");
    let ChildGrantOutcome::Issued { lineage } = first.clone() else {
        panic!("c1 is issued: {first:?}")
    };
    assert!(matches!(
        entered_issuance(&f, &entered, &["s"], "c2").issue_or_recover(),
        Ok(ChildGrantOutcome::Issued { .. })
    ));
    assert!(f.issued("c2").is_some());
    assert_eq!(
        entered_issuance(&f, &entered, &["s"], "c1").issue_or_recover(),
        Ok(ChildGrantOutcome::Recovered {
            lineage: lineage.clone()
        }),
        "a retried intent is recovered"
    );
    assert!(f.resolves(&lineage, "helper"), "c1 is the helper's lineage");
    assert!(
        dispatch
            .context
            .enter_governed_operation(dispatch.call())
            .is_ok(),
        "an unreviewed entry spends nothing and can enter again"
    );
}

/// The native work owner runs a further step of an entered effect under its
/// own exclusion only while the complete current decision needs no more
/// review than the dispatch entered: an R2 decision runs and issues for an
/// R2 entry while the plain path, R1-only, refuses it; a decision raised to
/// R3 refuses an R2 entry before the step runs or anything is appended, and
/// an R3 entry covers it; every further original stays a conjunct.
#[test]
fn an_entered_step_runs_only_within_the_entered_tier_under_the_owners_exclusion() {
    use meerkat_authorization::grants::ChildGrantRefusal;
    use meerkat_authorization::work::FurtherOriginals;
    use meerkat_core::authorization::OperationReviewTier;
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;

    let tier = TierSet::at(0);
    let f = Fixture::new_with(tier.clone());
    f.source("s", ExecutionRestrictions::unrestricted());
    let further = FurtherOriginals {
        originals: retained_originals(&["s"]),
        source: Arc::new(SourceOriginalBasis::new(f.ledger.clone())),
    };
    let entered_run = |entered_tier| {
        let mut ran = false;
        let result = f.context.authorization().run_entered_if_current_within(
            f.setup.binding(),
            f.setup.validity().as_ref(),
            &further,
            &mut || ran = true,
            entered_tier,
        );
        (result, ran)
    };
    let plain_run = || {
        let mut ran = false;
        let result = f.context.authorization().run_if_current_within(
            f.setup.binding(),
            f.setup.validity().as_ref(),
            &further,
            &mut || ran = true,
        );
        (result, ran)
    };
    let issue = |name: &str, entered_tier| {
        f.context.authorization().issue_child_if_current_within(
            f.setup.binding(),
            f.setup.validity().as_ref(),
            &further,
            &f.intent(name),
            entered_tier,
        )
    };
    let denied = |result: &Result<(), meerkat_core::OperationAuthorizationError>| {
        matches!(
            result,
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::Denied
        )
    };

    // The owners now require R2, the review the dispatch entered.
    tier.set(1);
    assert_eq!(
        entered_run(OperationReviewTier::R2),
        (Ok(()), true),
        "an R2 decision runs for an R2 entry"
    );
    let (plain, ran) = plain_run();
    assert!(
        denied(&plain) && !ran,
        "the plain path stays R1-only: no review runs there"
    );
    let issued = issue("within-tier", OperationReviewTier::R2).expect("issued for an R2 entry");
    assert!(matches!(
        *issued
            .downcast::<Result<ChildGrantOutcome, ChildGrantRefusal>>()
            .expect("the grant owner's typed outcome"),
        Ok(ChildGrantOutcome::Issued { .. })
    ));

    // Raised to R3: more review than was entered.
    tier.set(2);
    let (over, ran) = entered_run(OperationReviewTier::R2);
    assert!(
        denied(&over) && !ran,
        "an R3 decision refuses an R2 entry before the step runs"
    );
    assert!(matches!(
        issue("over-tier", OperationReviewTier::R2),
        Err(meerkat_core::OperationAuthorizationError::Refused(_))
    ));
    assert!(f.issued("over-tier").is_none(), "nothing was issued");
    assert_eq!(
        entered_run(OperationReviewTier::R3),
        (Ok(()), true),
        "an R3 entry covers an R3 decision"
    );

    // Every further original stays a conjunct of the step.
    tier.set(1);
    f.ledger
        .cancel_operations(&namespace("s"), &work("s"), evidence("cancel"))
        .expect("the further original is withdrawn");
    let (withdrawn, ran) = entered_run(OperationReviewTier::R2);
    assert!(
        denied(&withdrawn) && !ran,
        "a withdrawn further original refuses the step"
    );
}

/// A step stays inside the window of the decision the dispatch entered: the
/// owners' fresh decision now runs to 10_000, but a step entered under a
/// decision usable until 300 runs at 200 and refuses at 300, for a run and
/// for a child issuance; the same step under the fresh window still runs, so
/// only the entered window refused it.
#[test]
fn an_entered_step_stays_inside_the_entered_window_when_the_owners_renew() {
    use meerkat_authorization::work::FurtherOriginals;
    use meerkat_core::authorization::{OperationReviewTier, OperationValidity};
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;

    let f = Fixture::new();
    f.source("s", ExecutionRestrictions::unrestricted());
    let further = FurtherOriginals {
        originals: retained_originals(&["s"]),
        source: Arc::new(SourceOriginalBasis::new(f.ledger.clone())),
    };
    let fresh = f.setup.validity().expect("the setup decision is bounded");
    assert!(fresh.expires_at_ms > 300, "the owners' window runs longer");
    let entered = OperationValidity {
        not_before_ms: fresh.not_before_ms,
        expires_at_ms: 300,
        deadline: fresh.deadline,
    };
    let run = |validity: &OperationValidity| {
        let mut ran = false;
        let result = f.context.authorization().run_entered_if_current_within(
            f.setup.binding(),
            Some(validity),
            &further,
            &mut || ran = true,
            OperationReviewTier::R2,
        );
        (result, ran)
    };

    f.clock.wall.store(200, Ordering::SeqCst);
    assert_eq!(run(&entered), (Ok(()), true), "inside the entered window");
    f.clock.wall.store(300, Ordering::SeqCst);
    let (late, ran) = run(&entered);
    assert!(
        matches!(
            late,
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::Denied
        ) && !ran,
        "the fresh decision's longer window does not extend the entered step"
    );
    assert!(
        matches!(
            f.context.authorization().issue_child_if_current_within(
                f.setup.binding(),
                Some(&entered),
                &further,
                &f.intent("late-child"),
                OperationReviewTier::R2,
            ),
            Err(meerkat_core::OperationAuthorizationError::Refused(_))
        ),
        "no child past the entered window"
    );
    assert!(f.issued("late-child").is_none(), "nothing was issued");
    assert_eq!(
        run(&fresh),
        (Ok(()), true),
        "the same step under the fresh window runs"
    );
}

/// A further original's grant revoked after the entry refuses every later
/// step, as a denial for that step.
#[tokio::test]
async fn an_entered_step_is_denied_once_a_further_grant_is_revoked() {
    let f = Fixture::new();
    f.source("s", ExecutionRestrictions::unrestricted());
    let dispatch = Dispatch::new(&f.context);
    let entered = dispatch.enter();
    assert!(
        matches!(
            entered_issuance(&f, &entered, &["s"], "before").issue_or_recover(),
            Ok(ChildGrantOutcome::Issued { .. })
        ),
        "current before the revocation"
    );
    let lineage = f
        .grants
        .issued_reference(&id("s"))
        .expect("lookup")
        .expect("the further original's grant");
    f.grants
        .revoke(&f.root, &lineage, &mut NoController)
        .expect("the further original's grant is revoked");
    assert_entered_steps_denied(&f, &entered, &["s"], "revoked").await;
}

/// A contributing original cancelled after the entry refuses every later
/// step, as a denial for that step.
#[tokio::test]
async fn an_entered_step_is_denied_once_a_contributing_original_is_cancelled() {
    let f = Fixture::new();
    f.source("s", ExecutionRestrictions::unrestricted());
    let dispatch = Dispatch::new(&f.context);
    let entered = dispatch.enter();
    assert!(
        matches!(
            entered_issuance(&f, &entered, &["s"], "before").issue_or_recover(),
            Ok(ChildGrantOutcome::Issued { .. })
        ),
        "current before the cancellation"
    );
    f.ledger
        .cancel_operations(&namespace("s"), &work("s"), evidence("withdrawn"))
        .expect("the contributing original is cancelled");
    assert_entered_steps_denied(&f, &entered, &["s"], "cancelled").await;
}

/// The decision's window passing after the entry refuses every later step,
/// as a denial for that step: a batch member's admitted ceiling ends the
/// decision at 500.
#[tokio::test]
async fn an_entered_step_is_denied_once_its_decision_window_passes() {
    let f = Fixture::new();
    f.source("s", ExecutionRestrictions::unrestricted());
    ledger_admit(&f, "b");
    let bounded = f
        .policy
        .work_context(
            Arc::from([
                f.child.clone(),
                Fixture::bounded(&association_of(&original("b"), &namespace("b")), 500),
            ]),
            scope(),
        )
        .expect("work context");
    let dispatch = Dispatch::new(&bounded);
    let entered = dispatch.enter();
    assert!(
        matches!(
            entered_issuance(&f, &entered, &["s"], "before").issue_or_recover(),
            Ok(ChildGrantOutcome::Issued { .. })
        ),
        "current inside the window"
    );
    f.clock.wall.store(500, Ordering::SeqCst);
    assert_entered_steps_denied(&f, &entered, &["s"], "expired").await;
}

/// A decision that needs more review than the dispatch entered refuses every
/// later step as a denial for that step, never as a review settlement: the
/// owners raise the decision to R2 after an unreviewed (R1) entry.
#[tokio::test]
async fn an_entered_step_is_denied_once_its_decision_needs_more_review() {
    use meerkat_core::authorization::OperationReviewTier;

    let tier = TierSet::at(0);
    let f = Fixture::new_with(tier.clone());
    f.source("s", ExecutionRestrictions::unrestricted());
    let dispatch = Dispatch::new(&f.context);
    let entered = dispatch.enter();
    assert_eq!(entered.entered_tier(), OperationReviewTier::R1);
    assert!(
        matches!(
            entered_issuance(&f, &entered, &["s"], "before").issue_or_recover(),
            Ok(ChildGrantOutcome::Issued { .. })
        ),
        "current at R1"
    );
    tier.set(1);
    assert_entered_steps_denied(&f, &entered, &["s"], "raised").await;
}

/// A refused step speaks only for itself: after c1 was issued through the
/// witness, a cancelled contributing original refuses c2 (`Denied`, nothing
/// appended, an operation denial with no not-dispatched certificate in
/// core), while c1's record is kept and its lineage still resolves.
#[tokio::test]
async fn a_refused_entered_step_leaves_the_earlier_steps_in_place() {
    use meerkat_authorization::work::FurtherOriginals;
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;

    let f = Fixture::new();
    f.source("s", ExecutionRestrictions::unrestricted());
    let dispatch = Dispatch::new(&f.context);
    let entered = dispatch.enter();
    let first = entered_issuance(&f, &entered, &["s"], "c1")
        .issue_or_recover()
        .expect("c1 issued");
    let ChildGrantOutcome::Issued { lineage } = first.clone() else {
        panic!("c1 is issued: {first:?}")
    };
    f.ledger
        .cancel_operations(&namespace("s"), &work("s"), evidence("withdrawn"))
        .expect("the contributing original is cancelled");

    assert_eq!(
        entered_issuance(&f, &entered, &["s"], "c2").issue_or_recover(),
        Err(HelperChildIssuanceRefusal::Denied),
        "c2 is refused for its own step"
    );
    assert_eq!(f.issued("c2"), None, "nothing appended for c2");
    let further = FurtherOriginals {
        originals: retained_originals(&["s"]),
        source: Arc::new(SourceOriginalBasis::new(f.ledger.clone())),
    };
    assert_step_denial(
        entered
            .issue_child_if_current_within(&further, &f.intent("c2"))
            .map(drop),
        "c2",
    );
    assert_eq!(
        f.issued("c1").as_ref(),
        lineage.last(),
        "c1's record is kept, not deleted or reissued"
    );
    assert!(
        f.resolves(&lineage, "helper"),
        "and c1 still resolves for the helper"
    );
}

/// An unreviewed dispatch context (no admitted review) cannot enter a
/// decision that requires review: minting a witness for an R2 or R3
/// decision is the entry's review settlement, so nothing entered and no
/// witness exists.
#[test]
fn an_unreviewed_dispatch_mints_no_witness_for_a_reviewed_decision() {
    for raised in [1_u8, 2] {
        let f = Fixture::new_with(TierSet::at(raised));
        let dispatch = Dispatch::new(&f.context);
        let minted = dispatch.context.enter_governed_operation(dispatch.call());
        match (raised, minted) {
            (
                1,
                Err(meerkat_core::ToolError::ReviewUnavailable {
                    kind: meerkat_core::ReviewUnavailableKind::UnsupportedEntry,
                }),
            )
            | (
                2,
                Err(meerkat_core::ToolError::ReviewUnsatisfied {
                    kind: meerkat_core::ReviewUnsatisfiedKind::HumanConsentRequired,
                    ..
                }),
            ) => {}
            (raised, other) => {
                panic!("an R{} decision does not enter here: {other:?}", raised + 1)
            }
        }
    }
}

/// The dispatch-path review classification: every unsatisfied review and
/// every unavailability final for the dispatch (deadline, missing reviewer,
/// unsupported entry, ended dispatch) is a denial; the review owner's own
/// unavailability (contention, a conflicting or unconfirmed store, a failed
/// reviewer, unreadable review material) and any error that is not a review
/// settlement are not.
#[test]
fn a_dispatch_review_settlement_is_a_denial_unless_its_owner_cannot_answer() {
    use crate::governed_realm::review_settlement_denies;
    use meerkat_core::authorization::{OperationRefusalKind, OperationRefused};
    use meerkat_core::{
        ReviewUnavailableKind as Unavailable, ReviewUnsatisfiedKind as Unsatisfied,
    };

    for kind in [
        Unsatisfied::Denied,
        Unsatisfied::Escalated,
        Unsatisfied::ContextChanged,
        Unsatisfied::HumanConsentRequired,
        Unsatisfied::AlreadyConsumed,
    ] {
        assert!(
            review_settlement_denies(&meerkat_core::ToolError::ReviewUnsatisfied {
                kind,
                approval_id: None,
            }),
            "{kind:?} is final for the dispatch"
        );
    }
    for kind in [
        Unavailable::DeadlineExpired,
        Unavailable::ReviewerMissing,
        Unavailable::UnsupportedEntry,
        Unavailable::DispatchEnded,
    ] {
        assert!(
            review_settlement_denies(&meerkat_core::ToolError::ReviewUnavailable { kind }),
            "{kind:?} is final for the dispatch"
        );
    }
    for kind in [
        Unavailable::OwnerUnavailable,
        Unavailable::ReviewerFailed,
        Unavailable::StoreConflict,
        Unavailable::StoreCommitUnknown,
        Unavailable::ContextUnavailable,
    ] {
        assert!(
            !review_settlement_denies(&meerkat_core::ToolError::ReviewUnavailable { kind }),
            "{kind:?}: the review owner cannot answer now, a later attempt may succeed"
        );
    }
    assert!(!review_settlement_denies(
        &meerkat_core::ToolError::OperationAuthorizationUnavailable
    ));
    assert!(
        !review_settlement_denies(&meerkat_core::ToolError::AuthorizationRefused {
            refusal: OperationRefused::new(OperationRefusalKind::ReprepareRequired),
        }),
        "an operation refusal is not a review settlement"
    );
}

/// The plain dispatch paths run no review under the graph lock, and a
/// decision that needs one is final for the dispatch: an R2 or R3 decision
/// refuses the dispatch-path fence and the dispatch-path child issuance as
/// `Denied` (a review this entry cannot carry), with nothing committed and
/// nothing appended.
#[tokio::test]
async fn a_reviewed_decision_on_the_plain_dispatch_paths_is_denied() {
    for raised in [1_u8, 2] {
        let f = Fixture::new_with(TierSet::at(raised));
        f.source("s", ExecutionRestrictions::unrestricted());
        let dispatch = Dispatch::new(&f.context);
        let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
        let key = WorkAdmissionKey::new(format!("helper-r{}", raised + 1)).expect("key");
        let fence = GovernedHelperAdmissionFence::from_dispatch(
            dispatch.call(),
            &dispatch.context,
            f.ledger.clone(),
            retained_originals(&["s"]),
            "default",
            WorkNamespace::default(),
            service
                .admission_identity(key.clone(), setup_request())
                .expect("identity"),
        )
        .expect("the dispatch's own operation builds the fence");
        assert!(
            service
                .create_idempotent_with_fence(key, setup_request(), fence.clone())
                .await
                .is_err()
        );
        assert_eq!(
            fence.refusal(),
            Some(HelperAdmissionRefusal::Denied),
            "fence at R{}",
            raised + 1
        );
        assert!(
            service
                .list(meerkat_workgraph::WorkItemFilter {
                    include_terminal: true,
                    ..Default::default()
                })
                .await
                .expect("list")
                .is_empty(),
            "nothing committed"
        );
        assert_eq!(
            GovernedHelperChildIssuance::from_dispatch(
                dispatch.call(),
                &dispatch.context,
                f.ledger.clone(),
                retained_originals(&["s"]),
                f.intent("dispatch-child"),
            )
            .expect("the dispatch's own operation builds the issuance")
            .issue_or_recover(),
            Err(HelperChildIssuanceRefusal::Denied),
            "issuance at R{}",
            raised + 1
        );
        assert_eq!(f.issued("dispatch-child"), None, "nothing appended");
    }
}

// ---------------------------------------------------------------------------
// Waiting issuance: another actor holds the grant publication's writer.
// ---------------------------------------------------------------------------

/// Another actor holding the grant publication's writer with an exclusion
/// that never publishes, on its own thread, until released.
struct HeldExclusion {
    release: std::sync::mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl HeldExclusion {
    /// Returns once the writer is held.
    fn begin(f: &Fixture) -> Self {
        let (held, holding) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let grants = Arc::clone(&f.grants);
        let thread = std::thread::spawn(move || {
            grants
                .try_with_owner_changes_excluded(|| {
                    held.send(()).expect("the hold is reported");
                    // Released, or the sender dropped: `recv` returns either way.
                    let _ = released.recv();
                })
                .expect("the writer is free");
        });
        holding.recv().expect("the writer is held");
        Self { release, thread }
    }

    fn release(self) {
        drop(self.release);
        self.thread.join().expect("the holder's thread");
    }
}

/// A dispatch's own child issuance and a step of its entered operation meet
/// another actor's held exclusion. The synchronous form answers
/// `Unavailable` at once, as before; the waiting form meets the hold and
/// waits for it (its first poll is pending), then issues the child once
/// the hold ends, and a retried intent recovers it.
#[tokio::test]
async fn a_waiting_child_issuance_waits_out_a_held_exclusion_then_issues() {
    let f = Fixture::new();
    f.source("s", ExecutionRestrictions::unrestricted());
    let dispatch = Dispatch::new(&f.context);
    let entered = dispatch.enter();
    let issuances = [
        (
            "dispatch-child",
            GovernedHelperChildIssuance::from_dispatch(
                dispatch.call(),
                &dispatch.context,
                f.ledger.clone(),
                retained_originals(&["s"]),
                f.intent("dispatch-child"),
            )
            .expect("the dispatch's own operation builds the issuance"),
        ),
        (
            "entered-child",
            entered_issuance(&f, &entered, &["s"], "entered-child"),
        ),
    ];
    for (name, issuance) in issuances {
        let holder = HeldExclusion::begin(&f);
        assert_eq!(
            issuance.issue_or_recover(),
            Err(HelperChildIssuanceRefusal::Unavailable),
            "{name}: the synchronous form answers at once"
        );
        let mut waiting = std::pin::pin!(issuance.issue_or_recover_after_owner_change());
        assert!(
            futures::poll!(waiting.as_mut()).is_pending(),
            "{name}: the waiting form met the hold and waits for it"
        );
        holder.release();
        assert!(
            matches!(waiting.await, Ok(ChildGrantOutcome::Issued { .. })),
            "{name}: issued once the hold ended"
        );
        assert!(f.issued(name).is_some(), "{name}: the child is retained");
        assert!(
            matches!(
                issuance.issue_or_recover_after_owner_change().await,
                Ok(ChildGrantOutcome::Recovered { .. })
            ),
            "{name}: a retried intent recovers it: nothing was appended twice"
        );
    }
}

// ---------------------------------------------------------------------------
// Waiting admission: the fence's keyed create, retried after contention.
// ---------------------------------------------------------------------------

mod admission_wait;
