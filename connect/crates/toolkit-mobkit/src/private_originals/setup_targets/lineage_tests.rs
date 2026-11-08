//! Real process-local grant observations, not input admission or a BOTH rerun.
//! A controlled native clock publishes only inside a resolution, following
//! Core's existing publication-during-clock-read regression seam.
use super::*;
use meerkat_authorization::{
    clock::{LocalAuthorizationClock, LocalAuthorizationTime, LocalClockError},
    grants::LocalGrantConfiguration,
};
use meerkat_authorization_contracts::constraints::LifetimeRestriction;
use meerkat_core::{PrincipalKind, TrustDomainId};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

struct PublishingClock {
    publication: LocalAuthorizationPublication,
    changes: AtomicUsize,
    calls: AtomicUsize,
    unavailable: AtomicBool,
    expire_on_change: AtomicBool,
    unix_ms: AtomicU64,
}
impl LocalAuthorizationClock for PublishingClock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(LocalClockError::Unavailable);
        }
        let unix_ms = self.unix_ms.load(Ordering::SeqCst);
        if self
            .changes
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            // Armed only after real root/child issuance has returned. No
            // publication or grant writer is held by the resolution's clock.
            let change = self.publication.begin_owner_change().unwrap();
            if self.expire_on_change.load(Ordering::SeqCst) {
                self.unix_ms.store(300, Ordering::SeqCst);
            }
            drop(change);
        }
        Ok(LocalAuthorizationTime {
            unix_ms,
            monotonic: meerkat_core::time_compat::Instant::now(),
        })
    }
}
struct Fixture {
    grants: LocalGrantAuthority,
    publication: LocalAuthorizationPublication,
    clock: Arc<PublishingClock>,
    lineage: Vec<GrantLineageRef>,
    executor: PrincipalRef,
    expected: ExecutionRestrictions,
}
fn principal(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("target-lineage-test").unwrap(),
    )
    .unwrap()
}
fn fixture() -> Fixture {
    let publication = LocalAuthorizationPublication::new();
    let clock = Arc::new(PublishingClock {
        publication: publication.clone(),
        changes: AtomicUsize::new(0),
        calls: AtomicUsize::new(0),
        unavailable: AtomicBool::new(false),
        expire_on_change: AtomicBool::new(false),
        unix_ms: AtomicU64::new(100),
    });
    let administrator = principal("administrator");
    let executor = principal("executor");
    let grants = LocalGrantAuthority::new(
        LocalGrantConfiguration {
            root: administrator.clone(),
            namespace: EvidenceId::new("target-lineage").unwrap(),
            generation: 1,
        },
        publication.clone(),
        clock.clone(),
    )
    .unwrap();
    let bounds = ExecutionRestrictions {
        lifetime: LifetimeRestriction::window(0, 200),
        ..ExecutionRestrictions::unrestricted()
    };
    let parent = grants
        .issue_root(
            &administrator,
            EvidenceId::new("parent").unwrap(),
            administrator.clone(),
            None,
            bounds.clone(),
        )
        .unwrap();
    let child = grants
        .issue_child(
            &administrator,
            &parent,
            EvidenceId::new("child").unwrap(),
            executor.clone(),
            bounds,
        )
        .unwrap();
    let lineage = vec![parent, child];
    let expected = grants
        .resolve_lineage(&lineage, &executor, None)
        .unwrap()
        .restrictions()
        .clone();
    clock.calls.store(0, Ordering::SeqCst);
    Fixture {
        grants,
        publication,
        clock,
        lineage,
        executor,
        expected,
    }
}
fn resolve(
    budget: &mut ChildResolutionBudget,
    f: &Fixture,
) -> Result<(), LineageResolutionRefusal> {
    budget.resolve(
        &f.grants,
        &f.publication,
        &f.lineage,
        &f.executor,
        None,
        &f.expected,
    )
}
fn unchanged_issued_rows(f: &Fixture) {
    for reference in &f.lineage {
        assert_eq!(
            f.grants
                .issued_reference(&reference.grant_id)
                .unwrap()
                .as_ref(),
            Some(reference)
        );
    }
}

#[test]
fn target_child_reobserves_a_changed_publication_without_reissuing() {
    let f = fixture();
    // The old standalone call rejects this exact transient owner change.
    f.clock.changes.store(1, Ordering::SeqCst);
    assert!(matches!(
        f.grants.resolve_lineage(&f.lineage, &f.executor, None),
        Err(GrantRefusal::Unavailable)
    ));
    f.clock.calls.store(0, Ordering::SeqCst);
    f.clock.changes.store(1, Ordering::SeqCst);
    let mut budget = ChildResolutionBudget::default();
    assert_eq!(resolve(&mut budget, &f), Ok(()));
    assert_eq!(f.clock.calls.load(Ordering::SeqCst), 2);
    assert_eq!(budget.rebuilds, 1);
    unchanged_issued_rows(&f);
}

#[test]
fn target_child_rebuild_budget_is_shared_without_rearming() {
    let f = fixture();
    let mut budget = ChildResolutionBudget::default();
    f.clock.changes.store(2, Ordering::SeqCst);
    assert_eq!(resolve(&mut budget, &f), Ok(()));
    let deadline = budget.deadline;
    assert_eq!(budget.rebuilds, 2);
    f.clock.calls.store(0, Ordering::SeqCst);
    f.clock.changes.store(2, Ordering::SeqCst);
    assert_eq!(
        resolve(&mut budget, &f),
        Err(LineageResolutionRefusal::Changed)
    );
    assert_eq!(budget.rebuilds, OWNER_CHANGE_FREE_REBUILDS);
    assert_eq!(budget.deadline, deadline);
    assert_eq!(f.clock.calls.load(Ordering::SeqCst), 2);
    unchanged_issued_rows(&f);
}

#[test]
fn target_child_fresh_read_rejects_expiry_instead_of_reusing_old_restrictions() {
    let f = fixture();
    f.clock.changes.store(1, Ordering::SeqCst);
    f.clock.expire_on_change.store(true, Ordering::SeqCst);
    assert_eq!(
        resolve(&mut ChildResolutionBudget::default(), &f),
        Err(LineageResolutionRefusal::Refused(GrantRefusal::Denied))
    );
    assert_eq!(f.clock.calls.load(Ordering::SeqCst), 2);
    unchanged_issued_rows(&f);
}

#[test]
fn target_child_permanent_unavailability_does_not_spend_a_rebuild() {
    let f = fixture();
    let mut budget = ChildResolutionBudget::default();
    f.clock.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        resolve(&mut budget, &f),
        Err(LineageResolutionRefusal::Refused(GrantRefusal::Unavailable))
    );
    assert_eq!(f.clock.calls.load(Ordering::SeqCst), 1);
    assert_eq!(budget.rebuilds, 0);
    assert!(budget.deadline.is_none());
    f.clock.unavailable.store(false, Ordering::SeqCst);
    f.clock.calls.store(0, Ordering::SeqCst);
    f.publication.poison();
    assert_eq!(
        resolve(&mut budget, &f),
        Err(LineageResolutionRefusal::Refused(GrantRefusal::Unavailable))
    );
    assert_eq!(f.clock.calls.load(Ordering::SeqCst), 0);
    assert_eq!(budget.rebuilds, 0);
    unchanged_issued_rows(&f);
}

#[test]
fn target_child_denial_and_wrong_bounds_remain_final() {
    let f = fixture();
    let mut budget = ChildResolutionBudget::default();
    assert_eq!(
        budget.resolve(
            &f.grants,
            &f.publication,
            &f.lineage,
            &principal("other"),
            None,
            &f.expected
        ),
        Err(LineageResolutionRefusal::Refused(GrantRefusal::Denied))
    );
    assert_eq!(
        resolve(
            &mut budget,
            &Fixture {
                expected: ExecutionRestrictions::unrestricted(),
                ..f
            }
        ),
        Err(LineageResolutionRefusal::Refused(GrantRefusal::Denied))
    );
    assert_eq!(budget.rebuilds, 0);
    assert!(budget.deadline.is_none());
}

#[test]
fn target_child_held_publication_returns_changed_at_the_original_wait_bound() {
    let f = fixture();
    // Keep an actual owner change open for this call. The wait's own deadline,
    // not a test sleep or a raced release, must make the negative settle.
    let change = f.publication.begin_owner_change().unwrap();
    let start = Instant::now();
    let mut budget = ChildResolutionBudget::default();
    assert_eq!(
        resolve(&mut budget, &f),
        Err(LineageResolutionRefusal::Changed)
    );
    assert!(start.elapsed() >= OWNER_CHANGE_WAIT_BOUND);
    assert_eq!(budget.rebuilds, 0);
    assert_eq!(f.clock.calls.load(Ordering::SeqCst), 0);
    drop(change);
    assert_eq!(resolve(&mut ChildResolutionBudget::default(), &f), Ok(()));
    unchanged_issued_rows(&f);
}
