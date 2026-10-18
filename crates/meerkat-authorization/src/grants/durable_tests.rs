//! Durable restart of the same grant owner, not new issuance or a cache.
#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use crate::clock::{LocalAuthorizationTime, LocalClockError};
use crate::publication::PublicationError;
use meerkat_authorization_contracts::constraints::{
    ActionRef, DelegationDepth, ExactRestriction, LifetimeRestriction,
};
use meerkat_authorization_contracts::grant_mutation::{
    ControllerCustodyRefusal, ControllerGrantMutationCustody,
};
use meerkat_core::time_compat::Instant;
use meerkat_core::{PrincipalKind, TrustDomainId};

struct Clock;
impl LocalAuthorizationClock for Clock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        Ok(LocalAuthorizationTime {
            unix_ms: 100,
            monotonic: Instant::now(),
        })
    }
}
fn who(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("durable").expect("domain"),
    )
    .expect("principal")
}
fn id(name: &str) -> EvidenceId {
    EvidenceId::new(name).expect("id")
}
fn configuration() -> LocalGrantConfiguration {
    LocalGrantConfiguration {
        root: who("root"),
        namespace: id("namespace"),
        generation: 1,
    }
}
fn durable(store: Arc<dyn GrantOwnerStore>) -> LocalGrantAuthority {
    LocalGrantAuthority::new_durable(
        configuration(),
        store,
        LocalAuthorizationPublication::new(),
        Arc::new(Clock),
    )
    .expect("new durable owner")
}
fn restart(store: Arc<dyn GrantOwnerStore>) -> Result<LocalGrantAuthority, GrantDurabilityError> {
    LocalGrantAuthority::restore(store, LocalAuthorizationPublication::new(), Arc::new(Clock))
}
fn issue(owner: &LocalGrantAuthority, name: &str) -> GrantLineageRef {
    owner
        .issue_root(
            &who("root"),
            id(name),
            who("holder"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("issue")
}
fn state(owner: &LocalGrantAuthority) -> dsl::GrantAuthorityMachineState {
    owner.owner.lock().expect("owner").state().clone()
}

/// Store with a scripted append outcome, standing in for real write failures.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Append {
    Acknowledge,
    /// Refused before any write: the change is certainly absent.
    RefuseBeforeWrite,
    /// The record landed but the caller saw an uncertain failure.
    UncertainLanded,
    /// Nothing landed and the caller saw an uncertain failure.
    UncertainLost,
}
struct Scripted {
    inner: InMemoryGrantOwnerStore,
    next: Mutex<Append>,
}
impl Scripted {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: InMemoryGrantOwnerStore::new(),
            next: Mutex::new(Append::Acknowledge),
        })
    }
    fn set(&self, next: Append) {
        *self.next.lock().expect("script") = next;
    }
}
impl GrantOwnerStore for Scripted {
    fn load(&self) -> Result<Option<DurableGrantOwner>, GrantStoreError> {
        self.inner.load()
    }
    fn create(&self, identity: &GrantOwnerIdentity) -> Result<(), GrantStoreError> {
        self.inner.create(identity)
    }
    fn release(&self) {
        self.inner.release();
    }
    fn append(&self, change: &GrantOwnerChange) -> Result<(), GrantStoreError> {
        match *self.next.lock().expect("script") {
            Append::Acknowledge => self.inner.append(change),
            Append::RefuseBeforeWrite => Err(GrantStoreError::Refused),
            Append::UncertainLanded => {
                self.inner.append(change)?;
                Err(GrantStoreError::Uncertain)
            }
            Append::UncertainLost => Err(GrantStoreError::Uncertain),
        }
    }
}
fn try_issue(owner: &LocalGrantAuthority, name: &str) -> Result<GrantLineageRef, GrantRefusal> {
    owner.issue_root(
        &who("root"),
        id(name),
        who("holder"),
        None,
        ExecutionRestrictions::unrestricted(),
    )
}

#[test]
fn restored_owner_keeps_identity_records_and_revocations() {
    let store = Arc::new(InMemoryGrantOwnerStore::new());
    let owner = durable(store.clone());
    let allowed = issue(&owner, "allowed");
    let revoked = issue(&owner, "revoked");
    owner
        .revoke(&who("root"), &revoked, &mut IsolatedGrantTestCustody)
        .expect("revoke");
    let before = state(&owner);
    drop(owner);

    let restored = restart(store).expect("restore same owner");
    assert_eq!(
        state(&restored),
        before,
        "identity, incarnation and revision"
    );
    let resolved = restored
        .resolve_lineage(std::slice::from_ref(&allowed), &who("holder"), None)
        .expect("original reference resolves unchanged");
    assert_eq!(resolved.check_current(), Ok(()));
    assert!(matches!(
        restored.resolve_lineage(std::slice::from_ref(&revoked), &who("holder"), None),
        Err(GrantRefusal::Denied)
    ));
    assert!(
        matches!(
            restored.issue_root(
                &who("root"),
                id("revoked"),
                who("holder"),
                None,
                ExecutionRestrictions::unrestricted(),
            ),
            Err(GrantRefusal::Duplicate)
        ),
        "a revoked identifier is never reissued"
    );
    assert_eq!(
        restored.issued_reference(&id("revoked")),
        Ok(Some(revoked)),
        "the restored owner keeps the revoked grant's issued reference"
    );
}

#[test]
fn restored_owner_keeps_a_narrowed_three_level_chain() {
    let store = Arc::new(InMemoryGrantOwnerStore::new());
    let owner = durable(store.clone());
    let root = owner
        .issue_root(
            &who("root"),
            id("root-grant"),
            who("holder"),
            None,
            ExecutionRestrictions {
                actions: ExactRestriction::exact([ActionRef {
                    feature: "files".into(),
                    action: "read".into(),
                }]),
                lifetime: LifetimeRestriction::window(1, 200),
                delegation_depth: DelegationDepth::remaining(2),
                ..ExecutionRestrictions::unrestricted()
            },
        )
        .expect("root");
    let child = owner
        .issue_child(
            &who("holder"),
            &root,
            id("child"),
            who("child"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("child");
    let leaf = owner
        .issue_child(
            &who("child"),
            &child,
            id("leaf"),
            who("leaf"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("leaf");
    let references = [root, child, leaf];
    let previous = owner
        .resolve_lineage(&references, &who("leaf"), None)
        .expect("original chain");
    drop(owner);

    let restored = restart(store).expect("restore");
    let current = restored
        .resolve_lineage(&references, &who("leaf"), None)
        .expect("same exact chain after restart");
    assert_eq!(current.restrictions(), previous.restrictions());
    assert_eq!(current.expires_at_ms(), 200);
    assert!(
        matches!(
            restored.resolve_lineage(&references[1..], &who("leaf"), None),
            Err(GrantRefusal::Denied)
        ),
        "parent dependencies are not flattened"
    );
}

fn tamper(
    store: &InMemoryGrantOwnerStore,
    edit: impl FnOnce(&mut GrantRecord),
) -> Arc<InMemoryGrantOwnerStore> {
    let mut retained = store.inspect().expect("owner");
    let record = retained
        .changes
        .iter_mut()
        .rev()
        .find_map(|change| match &mut change.kind {
            durable::GrantOwnerChangeKind::Issued { record } => Some(record),
            durable::GrantOwnerChangeKind::Revoked { .. } => None,
        })
        .expect("child row");
    edit(record);
    let copy = Arc::new(InMemoryGrantOwnerStore::new());
    copy.create(&retained.identity).expect("copy identity");
    for change in &retained.changes {
        copy.append(change).expect("copy change");
    }
    copy.release();
    copy
}

#[test]
fn restore_refuses_a_widened_or_tampered_persisted_child() {
    let store = Arc::new(InMemoryGrantOwnerStore::new());
    let owner = durable(store.clone());
    let parent = owner
        .issue_root(
            &who("root"),
            id("parent"),
            who("holder"),
            None,
            ExecutionRestrictions {
                actions: ExactRestriction::exact([ActionRef {
                    feature: "files".into(),
                    action: "read".into(),
                }]),
                ..ExecutionRestrictions::unrestricted()
            },
        )
        .expect("parent");
    owner
        .issue_child(
            &who("holder"),
            &parent,
            id("child"),
            who("child"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("narrowed child");
    drop(owner);
    restart(store.clone()).expect("untampered store restores");

    let widened = tamper(&store, |record| {
        record.restrictions = ExecutionRestrictions::unrestricted();
    });
    assert!(matches!(
        restart(widened),
        Err(GrantDurabilityError::RestartRequired)
    ));
    let substituted = tamper(&store, |record| {
        record.represented_subject = Some(principal(who("substituted")).expect("principal"));
    });
    assert!(matches!(
        restart(substituted),
        Err(GrantDurabilityError::RestartRequired)
    ));
    let reissued = tamper(&store, |record| {
        record.issuer = principal(who("root")).expect("principal");
    });
    assert!(matches!(
        restart(reissued),
        Err(GrantDurabilityError::RestartRequired)
    ));
}

#[test]
fn lost_store_is_not_replaced_by_a_new_root_issuance() {
    let store = Arc::new(InMemoryGrantOwnerStore::new());
    let owner = durable(store.clone());
    let original = issue(&owner, "activation");
    let revoked = issue(&owner, "revoked");
    owner
        .revoke(&who("root"), &revoked, &mut IsolatedGrantTestCustody)
        .expect("revoke");
    drop(owner);
    drop(store);

    let lost: Arc<InMemoryGrantOwnerStore> = Arc::new(InMemoryGrantOwnerStore::new());
    assert!(matches!(
        restart(lost.clone()),
        Err(GrantDurabilityError::Missing)
    ));
    // Booting issue_root from an activation record is new issuance under a
    // new incarnation. It does not restore the original or revoked grants.
    let replacement = durable(lost.clone());
    let reissued = issue(&replacement, "activation");
    assert_ne!(reissued, original);
    assert_ne!(
        reissued.authority_incarnation,
        original.authority_incarnation
    );
    for old in [original, revoked] {
        assert!(matches!(
            replacement.resolve_lineage(&[old], &who("holder"), None),
            Err(GrantRefusal::Denied)
        ));
    }
    assert!(
        matches!(
            LocalGrantAuthority::new_durable(
                configuration(),
                lost,
                LocalAuthorizationPublication::new(),
                Arc::new(Clock),
            ),
            Err(GrantDurabilityError::Occupied)
        ),
        "an existing owner is never overwritten"
    );
}

#[test]
fn acknowledged_change_survives_and_a_pre_write_refusal_is_absent() {
    let store = Scripted::new();
    let owner = durable(store.clone());
    let acknowledged = issue(&owner, "acknowledged");
    let before = state(&owner);
    let ((), stamp) = owner.publication.observe(|| ()).expect("stamp");

    store.set(Append::RefuseBeforeWrite);
    assert_eq!(try_issue(&owner, "refused"), Err(GrantRefusal::Unavailable));
    assert_eq!(state(&owner), before, "nothing installed");
    assert_eq!(stamp.check_current(), Ok(()), "nothing published");
    store.set(Append::Acknowledge);
    let later = issue(&owner, "later");
    let after = state(&owner);
    drop(owner);

    let restored = restart(store).expect("restore");
    assert_eq!(state(&restored), after);
    restored
        .resolve_lineage(&[acknowledged], &who("holder"), None)
        .expect("acknowledged change survives");
    restored
        .resolve_lineage(&[later], &who("holder"), None)
        .expect("later acknowledged change survives");
    assert!(
        !state(&restored).records.contains_key(&id("refused")),
        "only a pre-write refusal proves the change is absent"
    );
}

/// A resolution inside an enclosing observation reports failed durable
/// custody as unavailability, never as a concurrent change.
#[test]
fn an_observed_resolution_reports_failed_custody_as_unavailable() {
    let store = Scripted::new();
    let owner = durable(store.clone());
    let kept = issue(&owner, "kept");
    store.set(Append::UncertainLost);
    assert_eq!(
        try_issue(&owner, "uncertain"),
        Err(GrantRefusal::Unavailable)
    );
    assert_eq!(
        owner
            .resolve_lineage_observed(std::slice::from_ref(&kept), &who("holder"), None)
            .err(),
        Some(LineageResolutionRefusal::Refused(GrantRefusal::Unavailable))
    );
    assert_eq!(
        owner
            .resolve_lineage_ceiling_observed(std::slice::from_ref(&kept))
            .err(),
        Some(LineageResolutionRefusal::Refused(GrantRefusal::Unavailable))
    );
}

#[test]
fn uncertain_write_makes_the_owner_unavailable_and_invalidates_observations() {
    for outcome in [Append::UncertainLanded, Append::UncertainLost] {
        let store = Scripted::new();
        let owner = durable(store.clone());
        let kept = issue(&owner, "kept");
        let before = state(&owner);
        let resolved = owner
            .resolve_lineage(std::slice::from_ref(&kept), &who("holder"), None)
            .expect("prepared before failure");

        store.set(outcome);
        assert_eq!(
            try_issue(&owner, "uncertain"),
            Err(GrantRefusal::Unavailable)
        );
        assert_eq!(state(&owner), before, "the candidate is never installed");
        assert_eq!(
            resolved.check_current(),
            Err(PublicationError::Changed),
            "the availability change is published to earlier observations"
        );
        store.set(Append::Acknowledge);
        assert!(matches!(
            owner.resolve_lineage(std::slice::from_ref(&kept), &who("holder"), None),
            Err(GrantRefusal::Unavailable)
        ));
        assert_eq!(
            owner.revoke(&who("root"), &kept, &mut IsolatedGrantTestCustody),
            Err(GrantRefusal::Unavailable)
        );
        assert_eq!(try_issue(&owner, "after"), Err(GrantRefusal::Unavailable));
        drop(owner);

        // Restart restores whatever complete record the store retained.
        let restored = restart(store).expect("restore");
        restored
            .resolve_lineage(&[kept], &who("holder"), None)
            .expect("acknowledged change survives");
        let landed = state(&restored).records.contains_key(&id("uncertain"));
        assert_eq!(landed, outcome == Append::UncertainLanded);
        assert!(!state(&restored).records.contains_key(&id("after")));
    }
}

#[test]
fn restored_owner_preserves_the_unfinished_controller_revocation_veto() {
    struct Veto(Option<ControllerCustodyRefusal>);
    impl ControllerGrantMutationCustody for Veto {
        fn with_unreferenced_controller_grant<T, E>(
            &mut self,
            _reference: &GrantLineageRef,
            mutate: impl FnOnce() -> Result<T, E>,
        ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
            match self.0 {
                Some(refusal) => Err(refusal),
                None => Ok(mutate()),
            }
        }
    }
    let store = Arc::new(InMemoryGrantOwnerStore::new());
    let owner = durable(store.clone());
    let controller = issue(&owner, "controller");
    drop(owner);

    let restored = restart(store.clone()).expect("restore");
    let changes = store.inspect().expect("owner").changes.len();
    let ((), stamp) = restored.publication.observe(|| ()).expect("stamp");
    assert_eq!(
        restored.revoke(
            &who("root"),
            &controller,
            &mut Veto(Some(ControllerCustodyRefusal::ControllerInUse))
        ),
        Err(GrantRefusal::ControllerInUse)
    );
    assert_eq!(stamp.check_current(), Ok(()));
    assert_eq!(
        store.inspect().expect("owner").changes.len(),
        changes,
        "a vetoed revocation appends nothing"
    );
    drop(restored);

    let again = restart(store.clone()).expect("restore again");
    again
        .resolve_lineage(std::slice::from_ref(&controller), &who("holder"), None)
        .expect("vetoed grant stays usable");
    again
        .revoke(&who("root"), &controller, &mut Veto(None))
        .expect("permitted revocation");
    drop(again);
    let last = restart(store).expect("restore after revoke");
    assert!(matches!(
        last.resolve_lineage(&[controller], &who("holder"), None),
        Err(GrantRefusal::Denied)
    ));
}

#[test]
fn file_store_round_trips_and_truncates_only_a_torn_tail() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("grant-owner.jsonl");
    let store = Arc::new(FileGrantOwnerStore::new(&path));
    assert_eq!(store.durability(), GrantStoreDurability::OsCrash);
    let owner = durable(store);
    let kept = issue(&owner, "kept");
    let revoked = issue(&owner, "revoked");
    owner
        .revoke(&who("root"), &revoked, &mut IsolatedGrantTestCustody)
        .expect("revoke");
    let before = state(&owner);
    assert!(
        matches!(
            restart(Arc::new(FileGrantOwnerStore::new(&path))),
            Err(GrantDurabilityError::StoreUnavailable)
        ),
        "the live store is the exclusive writer"
    );
    drop(owner);
    let acknowledged = std::fs::read(&path).expect("read");
    assert_eq!(
        acknowledged.last(),
        Some(&b'\n'),
        "every acknowledged line is complete"
    );

    // A torn tail is an incomplete record. It is never restored, and it is
    // physically truncated under the exclusive lock before any later append.
    let mut torn = acknowledged.clone();
    torn.extend_from_slice(b"{\"change\":{\"revision\":");
    std::fs::write(&path, &torn).expect("torn write");
    let restored = restart(Arc::new(FileGrantOwnerStore::new(&path))).expect("restore");
    assert_eq!(state(&restored), before);
    assert_eq!(std::fs::read(&path).expect("read"), acknowledged);
    restored
        .resolve_lineage(&[kept], &who("holder"), None)
        .expect("kept");
    assert!(matches!(
        restored.resolve_lineage(&[revoked], &who("holder"), None),
        Err(GrantRefusal::Denied)
    ));
    let later = issue(&restored, "later");
    drop(restored);
    let again = restart(Arc::new(
        FileGrantOwnerStore::new(&path).with_durability(GrantStoreDurability::ProcessCrash),
    ))
    .expect("restore after append");
    again
        .resolve_lineage(&[later], &who("holder"), None)
        .expect("append after truncation is acknowledged");
    drop(again);

    // A complete but corrupt line is not silently skipped.
    let mut corrupt = std::fs::read(&path).expect("read");
    corrupt.extend_from_slice(b"not json\n");
    std::fs::write(&path, &corrupt).expect("corrupt");
    assert!(matches!(
        restart(Arc::new(FileGrantOwnerStore::new(&path))),
        Err(GrantDurabilityError::RestartRequired)
    ));
    assert!(matches!(
        LocalGrantAuthority::new_durable(
            configuration(),
            Arc::new(FileGrantOwnerStore::new(&path)),
            LocalAuthorizationPublication::new(),
            Arc::new(Clock),
        ),
        Err(GrantDurabilityError::Occupied)
    ));
    let names: Vec<_> = std::fs::read_dir(directory.path())
        .expect("dir")
        .map(|entry| entry.expect("entry").file_name())
        .collect();
    assert_eq!(names.len(), 1, "no staged identity file remains");
}

fn race_restores(store: Arc<dyn GrantOwnerStore>) {
    let owner = durable(store.clone());
    let base = issue(&owner, "base");
    drop(owner);

    let barrier = Arc::new(std::sync::Barrier::new(2));
    let racers: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                restart(store)
            })
        })
        .collect();
    let results: Vec<_> = racers
        .into_iter()
        .map(|racer| racer.join().expect("racer"))
        .collect();
    let mut winners: Vec<_> = results.into_iter().filter_map(Result::ok).collect();
    assert_eq!(
        winners.len(),
        1,
        "exactly one restored owner holds the lease"
    );
    let winner = winners.pop().expect("winner");
    assert!(matches!(
        restart(store.clone()),
        Err(GrantDurabilityError::StoreUnavailable)
    ));
    winner
        .revoke(&who("root"), &base, &mut IsolatedGrantTestCustody)
        .expect("only the lease holder mutates");
    drop(winner);

    let next = restart(store).expect("lease released with its owner");
    assert!(
        matches!(
            next.resolve_lineage(&[base], &who("holder"), None),
            Err(GrantRefusal::Denied)
        ),
        "the next owner continues from the winner's acknowledged revision"
    );
    issue(&next, "continued");
}

#[test]
fn racing_restores_of_one_store_leave_exactly_one_mutation_owner() {
    race_restores(Arc::new(InMemoryGrantOwnerStore::new()));
    let directory = tempfile::tempdir().expect("tempdir");
    race_restores(Arc::new(FileGrantOwnerStore::new(
        directory.path().join("grant-owner.jsonl"),
    )));
}

#[test]
fn a_second_file_opener_cannot_take_the_lease() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("grant-owner.jsonl");
    let owner = durable(Arc::new(FileGrantOwnerStore::new(&path)));
    issue(&owner, "held");
    let other = FileGrantOwnerStore::new(&path);
    assert_eq!(other.load(), Err(GrantStoreError::Unavailable));
    assert_eq!(
        other.append(&GrantOwnerChange {
            revision: 2,
            kind: durable::GrantOwnerChangeKind::Revoked {
                grant_id: id("held")
            },
        }),
        Err(GrantStoreError::Refused),
        "an opener without the lease is refused before writing"
    );
    drop(owner);
    let restored = restart(Arc::new(FileGrantOwnerStore::new(&path))).expect("restore");
    assert_eq!(state(&restored).revision, 1);
}

#[test]
fn a_second_live_owner_cannot_read_stale_grants() {
    let store = Arc::new(InMemoryGrantOwnerStore::new());
    let first = durable(store.clone());
    let revoked = issue(&first, "revoked");
    first
        .revoke(&who("root"), &revoked, &mut IsolatedGrantTestCustody)
        .expect("revoke");
    assert_eq!(store.load(), Err(GrantStoreError::Unavailable));
    assert!(
        matches!(
            restart(store.clone()),
            Err(GrantDurabilityError::StoreUnavailable)
        ),
        "no second live owner, so no stale read"
    );
    drop(first);
    let second = restart(store).expect("after the first owner is gone");
    assert!(matches!(
        second.resolve_lineage(&[revoked], &who("holder"), None),
        Err(GrantRefusal::Denied)
    ));
}

#[test]
fn file_create_holds_the_lease_before_the_identity_is_published() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("grant-owner.jsonl");
    let observed = Arc::new(Mutex::new(None));
    let seen = observed.clone();
    let store = FileGrantOwnerStore::new(&path).after_publish(move |published| {
        let other = FileGrantOwnerStore::new(published);
        let loaded = other.load();
        let appended = other.append(&GrantOwnerChange {
            revision: 1,
            kind: durable::GrantOwnerChangeKind::Revoked {
                grant_id: id("intruder"),
            },
        });
        *seen.lock().expect("seen") = Some((loaded, appended));
    });
    let owner = durable(Arc::new(store));
    let (loaded, appended) = observed
        .lock()
        .expect("observed")
        .take()
        .expect("second opener ran between publication and return");
    assert_eq!(loaded, Err(GrantStoreError::Unavailable));
    assert_eq!(appended, Err(GrantStoreError::Refused));
    let first = issue(&owner, "first");
    drop(owner);

    let restored = restart(Arc::new(FileGrantOwnerStore::new(&path))).expect("restore");
    assert_eq!(state(&restored).revision, 1, "exactly one revision 1");
    restored
        .resolve_lineage(&[first], &who("holder"), None)
        .expect("the creator's change is the only one");
}

#[test]
fn process_crash_file_is_restored_under_os_crash_durability() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("grant-owner.jsonl");
    let weak = FileGrantOwnerStore::new(&path).with_durability(GrantStoreDurability::ProcessCrash);
    let owner = durable(Arc::new(weak));
    let early = issue(&owner, "early");
    drop(owner);

    // Load under the stronger profile syncs the file and its name first;
    // in-process tests cannot observe that sync, only that the upgrade works.
    let strong = FileGrantOwnerStore::new(&path);
    assert_eq!(strong.durability(), GrantStoreDurability::OsCrash);
    let restored = restart(Arc::new(strong)).expect("upgrade on load");
    restored
        .resolve_lineage(&[early], &who("holder"), None)
        .expect("weakly written change restored");
    let late = issue(&restored, "late");
    drop(restored);
    restart(Arc::new(FileGrantOwnerStore::new(&path)))
        .expect("restore")
        .resolve_lineage(&[late], &who("holder"), None)
        .expect("strongly acknowledged change");
}

/// Append that waits for the test to release it, then reports an uncertain
/// outcome without writing.
struct HeldAppend {
    inner: InMemoryGrantOwnerStore,
    hold: Mutex<
        Option<(
            std::sync::mpsc::SyncSender<()>,
            std::sync::mpsc::Receiver<()>,
        )>,
    >,
}
impl GrantOwnerStore for HeldAppend {
    fn load(&self) -> Result<Option<DurableGrantOwner>, GrantStoreError> {
        self.inner.load()
    }
    fn create(&self, identity: &GrantOwnerIdentity) -> Result<(), GrantStoreError> {
        self.inner.create(identity)
    }
    fn release(&self) {
        self.inner.release();
    }
    fn append(&self, change: &GrantOwnerChange) -> Result<(), GrantStoreError> {
        let held = self.hold.lock().expect("hold").take();
        match held {
            Some((entered, release)) => {
                entered.send(()).expect("entered");
                release.recv().expect("released");
                Err(GrantStoreError::Uncertain)
            }
            None => self.inner.append(change),
        }
    }
}

#[test]
fn a_lookup_waiting_behind_an_uncertain_append_is_unavailable() {
    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let store = Arc::new(HeldAppend {
        inner: InMemoryGrantOwnerStore::new(),
        hold: Mutex::new(None),
    });
    let owner = durable(store.clone());
    *store.hold.lock().expect("hold") = Some((entered_tx, release_rx));
    std::thread::scope(|scope| {
        let issuing = scope.spawn(|| try_issue(&owner, "held"));
        entered_rx.recv().expect("the append is held");
        // The lookup reaches the owner mutex while the issuance holds it,
        // before the uncertain outcome is known; only then is the append
        // released.
        let (boundary_tx, boundary_rx) = std::sync::mpsc::sync_channel(1);
        let looking = &owner;
        let lookup = scope.spawn(move || {
            super::test_boundary::on_lookup(move || {
                boundary_tx.send(()).expect("boundary acknowledged");
            });
            looking.issued_reference(&id("held"))
        });
        boundary_rx
            .recv()
            .expect("the lookup reached the owner mutex");
        release_tx.send(()).expect("release");
        assert_eq!(
            issuing.join().expect("issuing thread"),
            Err(GrantRefusal::Unavailable)
        );
        assert_eq!(
            lookup.join().expect("lookup thread"),
            Err(GrantRefusal::Unavailable),
            "never a stale answer from memory the uncertain append left unchanged"
        );
    });
}

/// A conjunct window open far beyond the fixture clock.
fn open() -> ChildGrantValidity {
    ChildGrantValidity {
        not_before_ms: 0,
        expires_at_ms: u64::MAX,
        deadline: Instant::now() + std::time::Duration::from_secs(3600),
    }
}

fn child_intent(parent: &GrantLineageRef, name: &str) -> ChildGrantIntent {
    ChildGrantIntent {
        issuer: who("holder"),
        parent_lineage: vec![parent.clone()],
        id: id(name),
        grantee: who("helper"),
        represented_subject: None,
        requested: ExecutionRestrictions::unrestricted(),
    }
}

/// The child entry under one reservation: a refused or unavailable
/// conjunct, or a conjunct whose answer no longer holds at the final clock
/// read, appends nothing; an id that names another grant is refused, never
/// recovered; the intended child is issued once and recovered by the same
/// intent without another append.
#[test]
fn a_child_intent_is_issued_once_only_within_its_conjunct() {
    let owner = durable(Arc::new(InMemoryGrantOwnerStore::new()));
    let parent = issue(&owner, "parent");
    let before = state(&owner);
    for (conjunct, expected) in [
        (Err(GrantRefusal::Denied), ChildGrantRefusal::Denied),
        (
            Err(GrantRefusal::Unavailable),
            ChildGrantRefusal::Unavailable,
        ),
        // The fixture clock reads 100: a window ending at 100 has expired.
        (
            Ok(ChildGrantValidity {
                expires_at_ms: 100,
                ..open()
            }),
            ChildGrantRefusal::Denied,
        ),
        // A window that has not started.
        (
            Ok(ChildGrantValidity {
                not_before_ms: 101,
                ..open()
            }),
            ChildGrantRefusal::Denied,
        ),
        // The monotonic deadline has passed although the wall clock is
        // inside the window.
        (
            Ok(ChildGrantValidity {
                deadline: Instant::now(),
                ..open()
            }),
            ChildGrantRefusal::Denied,
        ),
    ] {
        assert_eq!(
            owner.issue_or_recover_child_under_exclusion(&child_intent(&parent, "child"), || {
                conjunct
            }),
            Err(expected)
        );
        assert_eq!(state(&owner), before, "nothing appended");
    }

    let finite = ChildGrantIntent {
        requested: ExecutionRestrictions {
            lifetime: LifetimeRestriction::window(0, 10_000),
            ..ExecutionRestrictions::unrestricted()
        },
        ..child_intent(&parent, "finite")
    };
    assert_eq!(
        owner.issue_or_recover_child_under_exclusion(&finite, || Ok(open())),
        Err(ChildGrantRefusal::Denied),
        "a child no controller recovery could use is refused before append"
    );
    assert_eq!(state(&owner), before);

    owner
        .issue_child(
            &who("holder"),
            &parent,
            id("other"),
            who("someone-else"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("another child");
    let taken = state(&owner);
    assert_eq!(
        owner
            .issue_or_recover_child_under_exclusion(&child_intent(&parent, "other"), || Ok(open())),
        Err(ChildGrantRefusal::Denied),
        "an id naming another grant is never recovered"
    );
    assert_eq!(state(&owner), taken);

    let issued = owner
        .issue_or_recover_child_under_exclusion(&child_intent(&parent, "child"), || {
            Ok(ChildGrantValidity {
                expires_at_ms: 101,
                ..open()
            })
        })
        .expect("issued");
    let ChildGrantOutcome::Issued { lineage } = issued else {
        panic!("first append issues")
    };
    owner
        .resolve_controller_lineage(&lineage, &who("helper"), None)
        .expect("the helper's controller lineage");
    let landed = state(&owner);
    assert_eq!(
        owner
            .issue_or_recover_child_under_exclusion(&child_intent(&parent, "child"), || Ok(open())),
        Ok(ChildGrantOutcome::Recovered {
            lineage: lineage.clone()
        })
    );
    assert_eq!(state(&owner), landed, "recovery appends nothing");
    assert_eq!(
        owner.issue_or_recover_child_under_exclusion(&child_intent(&parent, "child"), || Err(
            GrantRefusal::Denied
        )),
        Err(ChildGrantRefusal::Denied),
        "a retained child is not usable without its current conjunct"
    );
    assert_eq!(
        owner
            .issued_reference(&id("child"))
            .expect("lookup")
            .as_ref(),
        lineage.last(),
        "its historical reference stays"
    );

    owner
        .revoke(&who("root"), &parent, &mut IsolatedGrantTestCustody)
        .expect("revoke the parent");
    let revoked = state(&owner);
    assert_eq!(
        owner.issue_or_recover_child_under_exclusion(&child_intent(&parent, "late"), || Ok(open())),
        Err(ChildGrantRefusal::Denied),
        "a revoked parent issues nothing"
    );
    assert_eq!(state(&owner), revoked);
}

/// An uncertain child append is typed Uncertain, not relabelled; the owner
/// is unavailable until restored. From the restored owner the same intent
/// recovers a landed child without another append, or issues exactly the
/// same id when it was lost; a landed child whose conjunct no longer holds
/// is refused without being deleted or reissued.
#[test]
fn an_uncertain_child_append_is_recovered_by_the_same_intent() {
    for outcome in [Append::UncertainLanded, Append::UncertainLost] {
        let store = Scripted::new();
        let owner = durable(store.clone());
        let parent = issue(&owner, "parent");
        store.set(outcome);
        assert_eq!(
            owner.issue_or_recover_child_under_exclusion(&child_intent(&parent, "child"), || Ok(
                open()
            )),
            Err(ChildGrantRefusal::Uncertain)
        );
        store.set(Append::Acknowledge);
        assert_eq!(
            owner.issued_reference(&id("child")),
            Err(GrantRefusal::Unavailable),
            "the failed live owner cannot say"
        );
        assert_eq!(
            owner.issue_or_recover_child_under_exclusion(&child_intent(&parent, "child"), || Ok(
                open()
            )),
            Err(ChildGrantRefusal::Unavailable)
        );
        drop(owner);

        let restored = restart(store).expect("restore");
        let retained = state(&restored);
        let landed = retained.records.contains_key(&id("child"));
        assert_eq!(landed, outcome == Append::UncertainLanded);
        if landed {
            assert_eq!(
                restored.issue_or_recover_child_under_exclusion(
                    &child_intent(&parent, "child"),
                    || { Err(GrantRefusal::Denied) }
                ),
                Err(ChildGrantRefusal::Denied),
                "a withdrawn source refuses usable recovery"
            );
            assert!(
                restored
                    .issued_reference(&id("child"))
                    .expect("lookup")
                    .is_some(),
                "the historical record stays"
            );
        }
        let recovered = restored
            .issue_or_recover_child_under_exclusion(&child_intent(&parent, "child"), || Ok(open()))
            .expect("same intent");
        if landed {
            assert!(matches!(recovered, ChildGrantOutcome::Recovered { .. }));
            assert_eq!(state(&restored), retained, "no second append");
        } else {
            assert!(matches!(recovered, ChildGrantOutcome::Issued { .. }));
            assert_eq!(state(&restored).revision, retained.revision + 1);
        }
        assert_eq!(
            restored
                .issued_reference(&id("child"))
                .expect("lookup")
                .as_ref(),
            recovered.lineage().last(),
            "exactly the intended id, never a replacement"
        );
        restored
            .resolve_controller_lineage(recovered.lineage(), &who("helper"), None)
            .expect("usable now");
    }
}
