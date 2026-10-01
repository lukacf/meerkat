#![allow(clippy::expect_used)]

use std::sync::atomic::{AtomicU64, Ordering};

use crate::clock::LocalClockError;
use meerkat_authorization_contracts::constraints::{
    ActionRef, DelegationDepth, DepthBound, ExactRestriction, LifetimeRestriction, SetBound,
    UnresolvedConstraint,
};
use meerkat_core::time_compat::Instant;
use meerkat_core::{PrincipalKind, TrustDomainId};

use super::*;
use crate::clock::LocalAuthorizationTime;

fn who(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("grant-fixture").expect("domain"),
    )
    .expect("principal")
}
fn id(name: &str) -> EvidenceId {
    EvidenceId::new(name).expect("id")
}

#[test]
fn controller_lineage_requires_declared_unrestricted_ancestors_not_max_finite_expiry() {
    let fixture = Fixture::new();
    let finite = fixture
        .authority
        .issue_root(
            &who("root"),
            id("finite-max"),
            who("executor"),
            None,
            restrictions(0, 0, u64::MAX),
        )
        .expect("finite ordinary grant");
    assert!(
        fixture
            .authority
            .resolve_lineage(std::slice::from_ref(&finite), &who("executor"), None)
            .is_ok()
    );
    assert!(
        fixture
            .authority
            .resolve_controller_lineage(&[finite], &who("executor"), None)
            .is_err()
    );
    let root = fixture
        .authority
        .issue_root(
            &who("root"),
            id("controller-root"),
            who("delegator"),
            None,
            ExecutionRestrictions {
                delegation_depth: DelegationDepth::remaining(1),
                ..ExecutionRestrictions::unrestricted()
            },
        )
        .expect("unrestricted controller root");
    let child = fixture
        .authority
        .issue_child(
            &who("delegator"),
            &root,
            id("controller-leaf"),
            who("executor"),
            ExecutionRestrictions {
                delegation_depth: DelegationDepth::remaining(0),
                ..ExecutionRestrictions::unrestricted()
            },
        )
        .expect("controller child");
    assert!(
        fixture
            .authority
            .resolve_controller_lineage(&[root, child], &who("executor"), None)
            .is_ok()
    );
}
fn action(name: &str) -> ActionRef {
    ActionRef {
        feature: "files".into(),
        action: name.into(),
    }
}
fn restrictions(depth: u32, start: u64, end: u64) -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact([action("read"), action("write")]),
        lifetime: LifetimeRestriction::window(start, end),
        delegation_depth: DelegationDepth::remaining(depth),
        ..ExecutionRestrictions::unrestricted()
    }
}

struct Clock(AtomicU64);
impl LocalAuthorizationClock for Clock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        Ok(LocalAuthorizationTime {
            unix_ms: self.0.load(Ordering::SeqCst),
            monotonic: Instant::now(),
        })
    }
}
struct Fixture {
    authority: LocalGrantAuthority,
    clock: Arc<Clock>,
    publication: LocalAuthorizationPublication,
}
impl Fixture {
    fn new() -> Self {
        let clock = Arc::new(Clock(AtomicU64::new(100)));
        let publication = LocalAuthorizationPublication::new();
        let authority = LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: who("root"),
                namespace: id("namespace"),
                generation: 1,
            },
            publication.clone(),
            clock.clone(),
        )
        .expect("configure");
        Self {
            authority,
            clock,
            publication,
        }
    }
    fn root(&self) -> GrantLineageRef {
        self.authority
            .issue_root(
                &who("root"),
                id("root-grant"),
                who("delegator"),
                Some(who("represented-human")),
                restrictions(2, 100, 300),
            )
            .expect("root issuance")
    }
    fn three_levels(&self) -> Vec<GrantLineageRef> {
        let root = self.root();
        let child = self
            .authority
            .issue_child(
                &who("delegator"),
                &root,
                id("child"),
                who("second-delegator"),
                ExecutionRestrictions {
                    actions: ExactRestriction::exact([action("read"), action("delete")]),
                    lifetime: LifetimeRestriction::window(90, 250),
                    delegation_depth: DelegationDepth::remaining(9),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("child");
        let leaf = self
            .authority
            .issue_child(
                &who("second-delegator"),
                &child,
                id("leaf"),
                who("executor"),
                ExecutionRestrictions::unrestricted(),
            )
            .expect("leaf");
        vec![root, child, leaf]
    }
    fn resolve(&self, chain: &[GrantLineageRef]) -> Result<ResolvedGrant, GrantRefusal> {
        self.authority
            .resolve_lineage(chain, &who("executor"), Some(&who("represented-human")))
    }
}

#[test]
fn three_levels_use_exact_attenuation_and_zero_depth_only_forbids_children() {
    let fixture = Fixture::new();
    let chain = fixture.three_levels();
    let resolved = fixture.resolve(&chain).expect("current full chain");
    assert_eq!(
        resolved.restrictions().actions.bound(),
        &SetBound::Exact([action("read")].into())
    );
    assert_eq!(
        resolved.restrictions().delegation_depth.bound(),
        DepthBound::Remaining(0)
    );
    assert_eq!(resolved.expires_at_ms(), 250);
    assert!(matches!(
        fixture.authority.issue_child(
            &who("executor"),
            &chain[2],
            id("exhausted"),
            who("other"),
            ExecutionRestrictions::unrestricted()
        ),
        Err(GrantRefusal::Denied)
    ));
}

#[test]
fn revoked_ancestor_refuses_descendant_and_invalidates_existing_publication() {
    let fixture = Fixture::new();
    let chain = fixture.three_levels();
    let (_, stamp) = fixture
        .publication
        .observe(|| fixture.resolve(&chain).expect("before revoke"))
        .expect("coherent observation");
    fixture
        .authority
        .revoke(&who("root"), &chain[0], &mut IsolatedGrantTestCustody)
        .expect("revoke root grant");
    assert!(stamp.check_current().is_err());
    assert!(matches!(fixture.resolve(&chain), Err(GrantRefusal::Denied)));
    assert!(matches!(
        fixture.authority.issue_child(
            &who("second-delegator"),
            &chain[1],
            id("after-revoke"),
            who("other"),
            ExecutionRestrictions::unrestricted()
        ),
        Err(GrantRefusal::Denied)
    ));
}

#[test]
fn full_chain_is_checked_at_fresh_time_without_cached_valid_state() {
    let fixture = Fixture::new();
    let chain = fixture.three_levels();
    fixture.clock.0.store(99, Ordering::SeqCst);
    assert!(fixture.resolve(&chain).is_err());
    fixture.clock.0.store(100, Ordering::SeqCst);
    assert!(fixture.resolve(&chain).is_ok());
    fixture.clock.0.store(249, Ordering::SeqCst);
    assert!(fixture.resolve(&chain).is_ok());
    fixture.clock.0.store(250, Ordering::SeqCst);
    assert!(fixture.resolve(&chain).is_err());
    assert!(
        fixture
            .authority
            .issue_child(
                &who("delegator"),
                &chain[0],
                id("still-root-current"),
                who("another"),
                ExecutionRestrictions::unrestricted()
            )
            .is_ok()
    );
    fixture.clock.0.store(300, Ordering::SeqCst);
    assert!(
        fixture
            .authority
            .issue_child(
                &who("delegator"),
                &chain[0],
                id("expired-root"),
                who("another"),
                ExecutionRestrictions::unrestricted()
            )
            .is_err()
    );
}

#[test]
fn lineage_address_revision_and_completeness_are_exact() {
    let fixture = Fixture::new();
    let chain = fixture.three_levels();
    assert!(fixture.resolve(&chain[1..]).is_err());
    let mut reordered = chain.clone();
    reordered.swap(0, 1);
    assert!(fixture.resolve(&reordered).is_err());
    for change in 0..4 {
        let mut wrong = chain.clone();
        match change {
            0 => wrong[0].authority_namespace = id("other-namespace"),
            1 => wrong[1].authority_generation += 1,
            2 => wrong[2].issued_revision += 1,
            _ => wrong[2].root_authority = who("other-root"),
        }
        assert!(fixture.resolve(&wrong).is_err());
    }
}

#[test]
fn executor_represented_subject_and_issuer_are_distinct() {
    let fixture = Fixture::new();
    let chain = fixture.three_levels();
    assert!(
        fixture
            .authority
            .resolve_lineage(
                &chain,
                &who("represented-human"),
                Some(&who("represented-human"))
            )
            .is_err()
    );
    assert!(
        fixture
            .authority
            .resolve_lineage(&chain, &who("executor"), None)
            .is_err()
    );
    assert!(
        fixture
            .authority
            .resolve_lineage(&chain, &who("executor"), Some(&who("another-human")))
            .is_err()
    );
    assert!(
        fixture
            .authority
            .issue_child(
                &who("root"),
                &chain[1],
                id("root-not-holder"),
                who("other"),
                ExecutionRestrictions::unrestricted()
            )
            .is_err()
    );
    assert!(
        fixture
            .authority
            .revoke(&who("executor"), &chain[1], &mut IsolatedGrantTestCustody)
            .is_err()
    );
}

#[test]
fn issuance_and_revocation_are_immutable_with_no_id_reuse() {
    let fixture = Fixture::new();
    let root = fixture.root();
    assert!(
        fixture
            .authority
            .issue_root(
                &who("root"),
                root.grant_id.clone(),
                who("new-holder"),
                None,
                ExecutionRestrictions::unrestricted()
            )
            .is_err()
    );
    fixture
        .authority
        .revoke(&who("root"), &root, &mut IsolatedGrantTestCustody)
        .expect("revoke");
    let revision = fixture
        .authority
        .owner
        .lock()
        .expect("owner")
        .state()
        .revision;
    fixture
        .authority
        .revoke(&who("root"), &root, &mut IsolatedGrantTestCustody)
        .expect("idempotent revoke");
    assert_eq!(
        revision,
        fixture
            .authority
            .owner
            .lock()
            .expect("owner")
            .state()
            .revision
    );
    assert!(
        fixture
            .authority
            .issue_root(
                &who("root"),
                root.grant_id,
                who("new-holder"),
                None,
                ExecutionRestrictions::unrestricted()
            )
            .is_err()
    );
}

#[test]
fn unresolved_lifetime_and_depth_never_allow_use_or_child_issue() {
    for lifetime_unknown in [true, false] {
        let fixture = Fixture::new();
        let mut limits = restrictions(2, 100, 300);
        if lifetime_unknown {
            limits.lifetime = limits.lifetime.conjoin(&LifetimeRestriction::unresolved(
                UnresolvedConstraint::Unknown,
            ));
        } else {
            limits.delegation_depth =
                limits
                    .delegation_depth
                    .conjoin(&DelegationDepth::unresolved(
                        UnresolvedConstraint::Unavailable,
                    ));
        }
        let root = fixture
            .authority
            .issue_root(&who("root"), id("unknown"), who("executor"), None, limits)
            .expect("retained restrictive data");
        assert!(
            fixture
                .authority
                .resolve_lineage(std::slice::from_ref(&root), &who("executor"), None)
                .is_err()
        );
        assert!(
            fixture
                .authority
                .issue_child(
                    &who("executor"),
                    &root,
                    id("child"),
                    who("other"),
                    ExecutionRestrictions::unrestricted()
                )
                .is_err()
        );
    }
}

#[test]
fn qualified_principal_shape_does_not_accept_legacy_or_same_id_other_domain() {
    let legacy = PrincipalRef::new(PrincipalKind::ServiceAccount, "executor").expect("legacy");
    assert!(GrantPrincipal::new(legacy).is_err());
    let fixture = Fixture::new();
    let chain = fixture.three_levels();
    let other_domain = PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        "executor",
        TrustDomainId::new("other").expect("domain"),
    )
    .expect("principal");
    assert!(
        fixture
            .authority
            .resolve_lineage(&chain, &other_domain, Some(&who("represented-human")))
            .is_err()
    );
    assert!(
        fixture
            .authority
            .issue_root(
                &who("not-root"),
                id("forged-root"),
                who("executor"),
                None,
                ExecutionRestrictions::unrestricted()
            )
            .is_err()
    );
}

#[test]
fn direct_generated_child_rejects_foreign_parent_math_and_subject_substitution() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let mut owner = fixture.authority.owner.lock().expect("owner");
    let parent = exact_record(&owner, &root).expect("record").clone();
    let chain = chain(&owner, &parent).expect("chain");
    let derived = DerivedChildRestrictions::new(
        ExecutionRestrictions::unrestricted(),
        ExecutionRestrictions::unrestricted(),
    )
    .expect("foreign math");
    let mut record = GrantRecord {
        id: id("forged"),
        parent: Some(parent.id.clone()),
        issuer: parent.grantee.clone(),
        grantee: principal(who("executor")).expect("qualified"),
        represented_subject: parent.represented_subject.clone(),
        issued_revision: owner.state().revision + 1,
        restrictions: derived.effective().clone(),
    };
    let before = owner.state().clone();
    assert!(
        owner
            .apply(GrantAuthorityInput::IssueChild {
                actor: parent.grantee.clone(),
                record: record.clone(),
                derived,
                chain: chain.clone(),
                now_ms: 100
            })
            .is_err()
    );
    assert_eq!(owner.state(), &before);
    let derived = DerivedChildRestrictions::new(
        parent.restrictions.clone(),
        ExecutionRestrictions::unrestricted(),
    )
    .expect("actual math");
    record.restrictions = derived.effective().clone();
    record.represented_subject = None;
    assert!(
        owner
            .apply(GrantAuthorityInput::IssueChild {
                actor: parent.grantee,
                record,
                derived,
                chain,
                now_ms: 100
            })
            .is_err()
    );
    assert_eq!(owner.state(), &before);
}

#[test]
fn direct_generated_use_rejects_missing_revoked_and_substituted_ancestors() {
    let fixture = Fixture::new();
    let references = fixture.three_levels();
    let mut owner = fixture.authority.owner.lock().expect("owner");
    let leaf = exact_record(&owner, &references[2]).expect("leaf").clone();
    let actual = chain(&owner, &leaf).expect("chain");
    for mutation in 0..3 {
        let mut supplied = actual.clone();
        match mutation {
            0 => {
                supplied.remove(0);
            }
            1 => supplied[0].restrictions = ExecutionRestrictions::unrestricted(),
            _ => supplied[1].issuer = principal(who("forged-issuer")).expect("qualified"),
        }
        let before = owner.state().clone();
        assert!(
            owner
                .apply(GrantAuthorityInput::ResolveUse {
                    namespace: id("namespace"),
                    generation: 1,
                    executor: leaf.grantee.clone(),
                    represented_subject: leaf.represented_subject.clone(),
                    leaf: leaf.clone(),
                    chain: supplied,
                    now_ms: 100
                })
                .is_err()
        );
        assert_eq!(owner.state(), &before);
    }
}

#[test]
fn production_and_catalog_share_canonical_schema_and_data_types() {
    let actual = dsl::GrantAuthorityMachineState::schema();
    let expected = meerkat_machine_schema::catalog::dsl::dsl_grant_authority_production_schema();
    let actual =
        meerkat_machine_schema::catalog::dsl::grant_authority::schema_metadata().attach_to(actual);
    assert_eq!(actual, expected);
}

#[test]
fn unavailable_clock_refuses_use_and_child_without_changing_grant_state() {
    use std::sync::atomic::AtomicBool;

    struct AvailabilityClock(AtomicBool);
    impl LocalAuthorizationClock for AvailabilityClock {
        fn now(&self) -> Result<LocalAuthorizationTime, crate::clock::LocalClockError> {
            if self.0.load(Ordering::SeqCst) {
                return Err(crate::clock::LocalClockError::Unavailable);
            }
            Ok(LocalAuthorizationTime {
                unix_ms: 100,
                monotonic: Instant::now(),
            })
        }
    }

    let clock = Arc::new(AvailabilityClock(AtomicBool::new(false)));
    let publication = LocalAuthorizationPublication::new();
    let authority = LocalGrantAuthority::new(
        LocalGrantConfiguration {
            root: who("root"),
            namespace: id("namespace"),
            generation: 1,
        },
        publication.clone(),
        clock.clone(),
    )
    .expect("configure");
    let root = authority
        .issue_root(
            &who("root"),
            id("clock-root"),
            who("delegator"),
            None,
            restrictions(2, 100, 300),
        )
        .expect("root");
    let before = authority.owner.lock().expect("owner").state().clone();
    let (_, stamp) = publication.observe(|| ()).expect("before failure");
    clock.0.store(true, Ordering::SeqCst);

    assert!(matches!(
        authority.resolve_lineage(std::slice::from_ref(&root), &who("delegator"), None),
        Err(GrantRefusal::Unavailable)
    ));
    assert_eq!(authority.owner.lock().expect("owner").state(), &before);
    assert_eq!(stamp.check_current(), Ok(()));
    assert!(matches!(
        authority.issue_child(
            &who("delegator"),
            &root,
            id("clock-child"),
            who("executor"),
            ExecutionRestrictions::unrestricted()
        ),
        Err(GrantRefusal::Unavailable)
    ));
    assert_eq!(authority.owner.lock().expect("owner").state(), &before);
    assert_eq!(
        stamp.check_current(),
        Err(crate::publication::PublicationError::Changed)
    );

    clock.0.store(false, Ordering::SeqCst);
    let child = authority
        .issue_child(
            &who("delegator"),
            &root,
            id("clock-child"),
            who("executor"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("same child ID remains available");
    assert!(
        authority
            .resolve_lineage(&[root, child], &who("executor"), None)
            .is_ok()
    );
}

#[test]
fn native_custody_veto_precedes_publication_and_grant_mutation() {
    use meerkat_authorization_contracts::grant_mutation::{
        ControllerCustodyRefusal, ControllerGrantMutationCustody,
    };

    struct Custody {
        expected: GrantLineageRef,
        refusal: Option<ControllerCustodyRefusal>,
        visits: usize,
        callbacks: usize,
    }
    impl ControllerGrantMutationCustody for Custody {
        fn with_unreferenced_grant<T, E>(
            &mut self,
            reference: &GrantLineageRef,
            mutate: impl FnOnce() -> Result<T, E>,
        ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
            assert_eq!(reference, &self.expected);
            self.visits += 1;
            if let Some(refusal) = self.refusal {
                return Err(refusal);
            }
            self.callbacks += 1;
            Ok(mutate())
        }
    }

    let fixture = Fixture::new();
    let root = fixture.root();
    let before = fixture
        .authority
        .owner
        .lock()
        .expect("owner")
        .state()
        .clone();
    let (_, stamp) = fixture.publication.observe(|| ()).expect("before veto");
    let mut custody = Custody {
        expected: root.clone(),
        refusal: None,
        visits: 0,
        callbacks: 0,
    };
    for (refusal, expected) in [
        (
            ControllerCustodyRefusal::ReferencedByUnfinishedWork,
            GrantRefusal::Denied,
        ),
        (
            ControllerCustodyRefusal::Unavailable,
            GrantRefusal::Unavailable,
        ),
    ] {
        custody.refusal = Some(refusal);
        assert_eq!(
            fixture.authority.revoke(&who("root"), &root, &mut custody),
            Err(expected)
        );
        assert_eq!(
            fixture.authority.owner.lock().expect("owner").state(),
            &before
        );
        assert_eq!(stamp.check_current(), Ok(()));
        assert_eq!(custody.callbacks, 0);
    }
    assert_eq!(custody.visits, 2);
    custody.refusal = None;
    fixture
        .authority
        .revoke(&who("root"), &root, &mut custody)
        .expect("permitted custody callback");
    assert_eq!(custody.callbacks, 1);
    assert_eq!(
        stamp.check_current(),
        Err(crate::publication::PublicationError::Changed)
    );
    assert!(matches!(
        fixture.authority.resolve_lineage(
            std::slice::from_ref(&root),
            &who("delegator"),
            Some(&who("represented-human"))
        ),
        Err(GrantRefusal::Denied)
    ));
}

#[test]
fn zero_issued_revision_is_valid_reference_data_but_not_an_issued_grant() {
    let fixture = Fixture::new();
    let root = fixture.root();
    let before = fixture
        .authority
        .owner
        .lock()
        .expect("owner")
        .state()
        .clone();
    let mut unissued = root.clone();
    unissued.issued_revision = 0;
    assert_eq!(unissued.validate(), Ok(()));
    assert!(matches!(
        fixture.authority.resolve_lineage(
            &[unissued],
            &who("delegator"),
            Some(&who("represented-human"))
        ),
        Err(GrantRefusal::Denied)
    ));
    assert_eq!(
        fixture.authority.owner.lock().expect("owner").state(),
        &before
    );
    assert!(
        fixture
            .authority
            .resolve_lineage(&[root], &who("delegator"), Some(&who("represented-human")))
            .is_ok()
    );
}

#[test]
fn decoded_candidate_requires_actual_retained_issuance_and_exact_row() {
    let fixture = Fixture::new();
    let reference = fixture.root();
    let mut owner = fixture.authority.owner.lock().expect("owner");
    let issued = exact_record(&owner, &reference)
        .expect("issued row")
        .clone();
    let wire = serde_json::to_value(&issued).expect("issued candidate wire");
    let before = owner.state().clone();

    for field in ["id", "issued_revision", "grantee"] {
        let mut unissued = wire.clone();
        unissued[field] = match field {
            "id" => serde_json::json!("never-issued"),
            "issued_revision" => serde_json::json!(0),
            _ => serde_json::to_value(principal(who("other-executor")).expect("principal"))
                .expect("principal wire"),
        };
        let candidate: GrantRecord = serde_json::from_value(unissued).expect("valid candidate");
        assert!(
            owner
                .apply(GrantAuthorityInput::ResolveUse {
                    namespace: reference.authority_namespace.clone(),
                    generation: reference.authority_generation,
                    executor: candidate.grantee.clone(),
                    represented_subject: candidate.represented_subject.clone(),
                    leaf: candidate.clone(),
                    chain: vec![candidate],
                    now_ms: 100,
                })
                .is_err(),
            "decoded {field} substitution is not retained issuance"
        );
        assert_eq!(owner.state(), &before);
    }

    let decoded: GrantRecord = serde_json::from_value(wire).expect("exact issued candidate");
    let resolved = owner
        .apply(GrantAuthorityInput::ResolveUse {
            namespace: reference.authority_namespace,
            generation: reference.authority_generation,
            executor: decoded.grantee.clone(),
            represented_subject: decoded.represented_subject.clone(),
            leaf: decoded.clone(),
            chain: vec![decoded.clone()],
            now_ms: 100,
        })
        .expect("exact retained issued row still resolves");
    assert!(resolved.effects().iter().any(|effect| {
        matches!(effect, GrantAuthorityEffect::UseResolved { leaf } if leaf == &decoded)
    }));
    assert_eq!(owner.state(), &before);
}
