//! C5 regression tests: authored before the isolated production patch.
#![allow(clippy::expect_used)]

use super::*;
use crate::clock::{LocalAuthorizationTime, LocalClockError};
use crate::publication::{LocalPublicationStamp, PublicationError};
use meerkat_core::time_compat::Instant;
use meerkat_core::{PrincipalKind, TrustDomainId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

fn who(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("publication-tests").expect("domain"),
    )
    .expect("principal")
}
fn id(name: &str) -> EvidenceId {
    EvidenceId::new(name).expect("id")
}
fn now() -> LocalAuthorizationTime {
    LocalAuthorizationTime {
        unix_ms: 100,
        monotonic: Instant::now(),
    }
}
struct Clock;
impl LocalAuthorizationClock for Clock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        Ok(now())
    }
}
fn authority(clock: Arc<dyn LocalAuthorizationClock>) -> Arc<LocalGrantAuthority> {
    Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: who("root"),
                namespace: id("grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            clock,
        )
        .expect("owner"),
    )
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
        .expect("actual issue")
}
fn stamp(owner: &LocalGrantAuthority) -> LocalPublicationStamp {
    owner.publication.observe(|| ()).expect("coherent stamp").1
}

#[test]
fn refused_root_and_duplicate_issue_preserve_existing_stamp_and_rows() {
    let owner = authority(Arc::new(Clock));
    let existing = issue(&owner, "existing");
    let before = stamp(&owner);
    let revision = owner.owner.lock().expect("owner").state().revision;
    assert!(matches!(
        owner.issue_root(
            &who("intruder"),
            id("denied"),
            who("holder"),
            None,
            ExecutionRestrictions::unrestricted()
        ),
        Err(GrantRefusal::Denied)
    ));
    assert_eq!(before.check_current(), Ok(()));
    assert!(matches!(
        owner.issue_root(
            &who("root"),
            id("existing"),
            who("holder"),
            None,
            ExecutionRestrictions::unrestricted()
        ),
        Err(GrantRefusal::Duplicate)
    ));
    assert_eq!(before.check_current(), Ok(()));
    // A caller who may not issue learns nothing about retained ids.
    assert!(matches!(
        owner.issue_root(
            &who("intruder"),
            id("existing"),
            who("holder"),
            None,
            ExecutionRestrictions::unrestricted()
        ),
        Err(GrantRefusal::Denied)
    ));
    assert_eq!(before.check_current(), Ok(()));
    let live = owner.owner.lock().expect("owner");
    assert_eq!(live.state().revision, revision);
    assert_eq!(live.state().records.len(), 1);
    assert!(exact_record(&live, &existing).is_ok());
    drop(live);
    let _accepted = issue(&owner, "accepted");
    assert_eq!(before.check_current(), Err(PublicationError::Changed));
}

#[test]
fn refused_child_and_revoke_preserve_stamp_then_real_mutations_invalidate() {
    let owner = authority(Arc::new(Clock));
    let parent = issue(&owner, "parent");
    let before = stamp(&owner);
    assert!(matches!(
        owner.issue_child(
            &who("intruder"),
            &parent,
            id("child"),
            who("leaf"),
            ExecutionRestrictions::unrestricted()
        ),
        Err(GrantRefusal::Denied)
    ));
    assert_eq!(before.check_current(), Ok(()));
    assert!(matches!(
        owner.revoke(&who("intruder"), &parent, &mut IsolatedGrantTestCustody),
        Err(GrantRefusal::Denied)
    ));
    assert_eq!(before.check_current(), Ok(()));
    let child = owner
        .issue_child(
            &who("holder"),
            &parent,
            id("child"),
            who("leaf"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("accepted child");
    assert_eq!(before.check_current(), Err(PublicationError::Changed));
    let before_revoke = stamp(&owner);
    owner
        .revoke(&who("root"), &child, &mut IsolatedGrantTestCustody)
        .expect("accepted revoke");
    assert_eq!(
        before_revoke.check_current(),
        Err(PublicationError::Changed)
    );
    let after_revoke = stamp(&owner);
    owner
        .revoke(&who("root"), &child, &mut IsolatedGrantTestCustody)
        .expect("idempotent revoke");
    assert_eq!(
        after_revoke.check_current(),
        Ok(()),
        "accepted canonical no-op changes no facts"
    );
}

struct GateClock {
    once: Mutex<Option<(SyncSender<()>, Receiver<()>)>>,
}
impl LocalAuthorizationClock for GateClock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        let gate = self.once.lock().expect("test gate").take();
        if let Some((entered, release)) = gate {
            entered
                .send(())
                .expect("actual child reached its clock under the owner lock");
            release
                .recv_timeout(Duration::from_secs(5))
                .expect("release child");
        }
        Ok(now())
    }
}

#[test]
fn accepted_change_cannot_return_new_owner_facts_under_the_old_stamp() {
    let (entered_tx, entered_rx) = sync_channel(1);
    let (release_tx, release_rx) = sync_channel(1);
    let owner = authority(Arc::new(GateClock {
        once: Mutex::new(Some((entered_tx, release_rx))),
    }));
    let parent = issue(&owner, "parent");
    let before = stamp(&owner);
    let writer_owner = Arc::clone(&owner);
    let writer = std::thread::spawn(move || {
        writer_owner.issue_child(
            &who("holder"),
            &parent,
            id("child"),
            who("leaf"),
            ExecutionRestrictions::unrestricted(),
        )
    });
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("actual child under owner and publication custody");
    // GateClock is invoked from the real child mutation before generated apply.
    // This is positive evidence of the critical section, not scheduler timing.
    let before_result = before.check_current();
    let (reader_entered_tx, reader_entered_rx) = sync_channel(1);
    let reader_owner = Arc::clone(&owner);
    let (saw_new_tx, saw_new_rx) = sync_channel(1);
    let reader = std::thread::spawn(move || {
        reader_owner.publication.observe(|| {
            // observe has already loaded its initial stamp before this signal.
            reader_entered_tx
                .send(())
                .expect("reader entered observation");
            let exists = reader_owner
                .owner
                .lock()
                .expect("actual owner read")
                .state()
                .records
                .contains_key(&id("child"));
            saw_new_tx.send(exists).expect("actual observed data");
            exists
        })
    });
    let reader_entered = reader_entered_rx.recv_timeout(Duration::from_secs(2));
    release_tx
        .send(())
        .expect("always release writer before assertions");
    let child = writer
        .join()
        .expect("writer thread")
        .expect("actual child accepted");
    let observed = reader.join().expect("reader thread");
    assert_eq!(
        before_result,
        Ok(()),
        "unpublished work does not invalidate old decisions"
    );
    reader_entered.expect("old-stamp reader reached the actual owner observation");
    assert_eq!(
        saw_new_rx.recv_timeout(Duration::from_secs(2)),
        Ok(true),
        "reader actually saw the accepted new row"
    );
    assert!(
        matches!(observed, Err(PublicationError::Changed)),
        "new facts cannot keep an old stamp"
    );
    assert_eq!(before.check_current(), Err(PublicationError::Changed));
    let (exists, fresh) = owner
        .publication
        .observe(|| exact_record(&owner.owner.lock().expect("owner"), &child).is_ok())
        .expect("new coherent owner observation");
    assert!(exists);
    assert_eq!(fresh.check_current(), Ok(()));
}

struct PanickingClock;
#[allow(clippy::panic)] // Exercise panic poisoning under actual owner custody.
impl LocalAuthorizationClock for PanickingClock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        panic!("deliberate test panic under mutation custody")
    }
}

#[test]
fn mutation_panic_never_keeps_a_successfully_current_stamp() {
    let owner = authority(Arc::new(PanickingClock));
    let parent = issue(&owner, "parent");
    let before = stamp(&owner);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        owner.issue_child(
            &who("holder"),
            &parent,
            id("child"),
            who("leaf"),
            ExecutionRestrictions::unrestricted(),
        )
    }));
    assert!(result.is_err());
    assert_eq!(before.check_current(), Err(PublicationError::Unavailable));
    assert!(matches!(
        owner.publication.observe(|| ()),
        Err(PublicationError::Unavailable)
    ));
}

#[test]
fn issued_reference_is_the_retained_reference_or_none() {
    let owner = authority(Arc::new(Clock));
    let issued = issue(&owner, "existing");
    let before = stamp(&owner);
    assert_eq!(
        owner.issued_reference(&id("existing")),
        Ok(Some(issued.clone()))
    );
    assert_eq!(owner.issued_reference(&id("absent")), Ok(None));
    // A read: nothing is published.
    assert_eq!(before.check_current(), Ok(()));
    // A revoked grant keeps its issued reference.
    owner
        .revoke(&who("root"), &issued, &mut IsolatedGrantTestCustody)
        .expect("revoke");
    assert_eq!(owner.issued_reference(&id("existing")), Ok(Some(issued)));
}

#[test]
fn a_child_under_a_used_id_is_a_duplicate_only_for_its_issuer() {
    let owner = authority(Arc::new(Clock));
    let parent = issue(&owner, "parent");
    let child = owner
        .issue_child(
            &who("holder"),
            &parent,
            id("child"),
            who("executor"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("child");
    let before = stamp(&owner);
    for used in ["child", "parent"] {
        assert!(matches!(
            owner.issue_child(
                &who("holder"),
                &parent,
                id(used),
                who("executor"),
                ExecutionRestrictions::unrestricted(),
            ),
            Err(GrantRefusal::Duplicate)
        ));
        assert!(matches!(
            owner.issue_child(
                &who("intruder"),
                &parent,
                id(used),
                who("executor"),
                ExecutionRestrictions::unrestricted(),
            ),
            Err(GrantRefusal::Denied)
        ));
    }
    assert_eq!(before.check_current(), Ok(()));
    assert_eq!(owner.issued_reference(&id("child")), Ok(Some(child)));
}

#[test]
fn an_intruder_revoke_is_denied_even_while_the_reservation_is_busy() {
    let owner = authority(Arc::new(Clock));
    let parent = issue(&owner, "parent");
    let child = owner
        .issue_child(
            &who("holder"),
            &parent,
            id("child"),
            who("executor"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("child");
    let busy = owner
        .publication
        .reserve_owner_change()
        .expect("hold the reservation");
    for grant in [&parent, &child] {
        assert_eq!(
            owner.revoke(&who("intruder"), grant, &mut IsolatedGrantTestCustody),
            Err(GrantRefusal::Denied)
        );
    }
    // Authorized callers (the root, the child's issuer) meet the busy
    // reservation: unavailable, nothing changed.
    assert_eq!(
        owner.revoke(&who("root"), &parent, &mut IsolatedGrantTestCustody),
        Err(GrantRefusal::Unavailable)
    );
    assert_eq!(
        owner.revoke(&who("holder"), &child, &mut IsolatedGrantTestCustody),
        Err(GrantRefusal::Unavailable)
    );
    drop(busy);
    owner
        .revoke(&who("holder"), &child, &mut IsolatedGrantTestCustody)
        .expect("the issuer revokes once the reservation is free");
    assert_eq!(
        owner.revoke(&who("intruder"), &parent, &mut IsolatedGrantTestCustody),
        Err(GrantRefusal::Denied)
    );
}

fn revoked(owner: &LocalGrantAuthority, grant: &GrantLineageRef) -> bool {
    owner
        .owner
        .lock()
        .expect("owner")
        .state()
        .revoked
        .contains(&grant.grant_id)
}

fn revision(owner: &LocalGrantAuthority) -> u64 {
    owner.owner.lock().expect("owner").state().revision
}

#[test]
fn an_expected_revoke_after_a_published_owner_change_is_refused_unchanged() {
    let owner = authority(Arc::new(Clock));
    let grant = issue(&owner, "grant");
    // The caller checks its view, then a real owner change on the same
    // publication completes before the caller revokes.
    let checked = stamp(&owner);
    let _other = issue(&owner, "other");
    let after_change = stamp(&owner);
    let before = revision(&owner);
    assert_eq!(
        owner.revoke_expecting(
            &who("root"),
            &grant,
            &checked,
            &mut IsolatedGrantTestCustody
        ),
        Err(ExpectedRevokeRefusal::NotCurrent)
    );
    assert!(!revoked(&owner, &grant), "the grant stays live");
    assert_eq!(revision(&owner), before, "no owner change");
    assert_eq!(after_change.check_current(), Ok(()), "nothing published");
}

#[test]
fn an_expected_revoke_refuses_a_current_stamp_of_an_unrelated_publication() {
    let owner = authority(Arc::new(Clock));
    let grant = issue(&owner, "grant");
    let own = stamp(&owner);
    let unrelated = LocalAuthorizationPublication::new();
    let ((), foreign) = unrelated.observe(|| ()).expect("unrelated stamp");
    assert_eq!(foreign.check_current(), Ok(()), "current on its own");
    let before = revision(&owner);
    assert_eq!(
        owner.revoke_expecting(
            &who("root"),
            &grant,
            &foreign,
            &mut IsolatedGrantTestCustody
        ),
        Err(ExpectedRevokeRefusal::NotCurrent)
    );
    assert!(!revoked(&owner, &grant), "the grant stays live");
    assert_eq!(revision(&owner), before, "no owner change");
    assert_eq!(own.check_current(), Ok(()), "nothing published");
}

#[test]
fn an_expected_revoke_with_a_current_stamp_revokes_exactly_once() {
    let owner = authority(Arc::new(Clock));
    let grant = issue(&owner, "grant");
    let current = stamp(&owner);
    // Every refusal of `revoke` keeps its meaning; nothing is published.
    assert_eq!(
        owner.revoke_expecting(
            &who("intruder"),
            &grant,
            &current,
            &mut IsolatedGrantTestCustody
        ),
        Err(ExpectedRevokeRefusal::Grant(GrantRefusal::Denied))
    );
    assert_eq!(current.check_current(), Ok(()));
    let before = revision(&owner);
    owner
        .revoke_expecting(
            &who("root"),
            &grant,
            &current,
            &mut IsolatedGrantTestCustody,
        )
        .expect("a current stamp of this publication revokes");
    assert!(revoked(&owner, &grant));
    assert_eq!(revision(&owner), before + 1, "one owner change");
    let after = stamp(&owner);
    assert_eq!(
        after.observation_sequence(),
        current.observation_sequence() + 2,
        "published once"
    );
    // The checked stamp is now stale and cannot revoke again.
    assert_eq!(
        owner.revoke_expecting(
            &who("root"),
            &grant,
            &current,
            &mut IsolatedGrantTestCustody
        ),
        Err(ExpectedRevokeRefusal::NotCurrent)
    );
    assert_eq!(revision(&owner), before + 1);
    assert_eq!(after.check_current(), Ok(()), "nothing published again");
}

fn open() -> ChildGrantValidity {
    ChildGrantValidity {
        not_before_ms: 0,
        expires_at_ms: u64::MAX,
        deadline: Instant::now() + Duration::from_secs(3600),
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
/// A host owner sharing `publication` panics inside its own entry lock while
/// holding the sentinel, and catches its own panic.
#[allow(clippy::panic)] // Deliberately poison the host owner's own lock.
fn caught_host_poison(publication: &LocalAuthorizationPublication, host: &Mutex<()>) {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _entry = host.lock().expect("host entry");
        let _sentinel = publication.poison_on_unwind();
        panic!("simulated host owner panic");
    }));
    assert!(caught.is_err());
    assert!(host.is_poisoned());
}

#[test]
fn a_caught_owner_poison_inside_an_excluded_operation_stays_poisoned() {
    let owner = authority(Arc::new(Clock));
    let before = stamp(&owner);
    let publication = owner.publication.clone();
    let host = Mutex::new(());
    owner
        .try_with_owner_changes_excluded(|| caught_host_poison(&publication, &host))
        .expect("the operation ran under the exclusion");
    assert_eq!(before.check_current(), Err(PublicationError::Unavailable));
    assert!(matches!(
        owner.publication.observe(|| ()),
        Err(PublicationError::Unavailable)
    ));
    assert!(matches!(
        owner.try_with_owner_changes_excluded(|| ()),
        Err(PublicationError::Unavailable)
    ));
}

/// A conjunct that poisons the publication on its own thread, here through
/// a caught host panic, is seen before the append: the issuance refuses
/// `Unavailable` and appends nothing, the poison stands, and after
/// reconstruction the same intent issues the child afresh.
#[test]
fn a_caught_owner_poison_inside_a_child_issuance_is_never_overwritten() {
    let mut owner = authority(Arc::new(Clock));
    let parent = issue(&owner, "parent");
    let before = stamp(&owner);
    let revision_before = revision(&owner);
    let publication = owner.publication.clone();
    let host = Mutex::new(());
    let intent = child_intent(&parent, "child");
    assert_eq!(
        owner.issue_or_recover_child_under_exclusion(&intent, || {
            caught_host_poison(&publication, &host);
            Ok(open())
        }),
        Err(ChildGrantRefusal::Unavailable),
        "a conjunct's own poison refuses the append"
    );
    assert_eq!(revision(&owner), revision_before, "nothing was appended");
    assert!(
        !owner
            .owner
            .lock()
            .expect("owner")
            .state()
            .records
            .contains_key(&intent.id),
        "no row is retained for the intent"
    );
    assert_eq!(
        before.check_current(),
        Err(PublicationError::Unavailable),
        "the poison stands"
    );
    assert!(matches!(
        owner.publication.observe(|| ()),
        Err(PublicationError::Unavailable)
    ));
    assert_eq!(
        owner.issue_or_recover_child_under_exclusion(&intent, || Ok(open())),
        Err(ChildGrantRefusal::Unavailable)
    );

    Arc::get_mut(&mut owner)
        .expect("the only reference to the owner")
        .reconstruct(LocalAuthorizationPublication::new())
        .expect("reconstructed under a new publication");
    let issued = owner
        .issue_or_recover_child_under_exclusion(&intent, || Ok(open()))
        .expect("issued afresh after reconstruction");
    assert!(
        matches!(issued, ChildGrantOutcome::Issued { .. }),
        "a fresh issuance, not a recovery: {issued:?}"
    );
    owner
        .resolve_controller_lineage(issued.lineage(), &who("helper"), None)
        .expect("the issued child is usable");
}

/// A child issued and published before a poison keeps its outcome: it is
/// unusable while poisoned and recovered by the same intent after
/// reconstruction.
#[test]
fn a_child_issued_before_an_owner_poison_is_recovered_after_reconstruction() {
    let mut owner = authority(Arc::new(Clock));
    let parent = issue(&owner, "parent");
    let intent = child_intent(&parent, "child");
    let issued = owner
        .issue_or_recover_child_under_exclusion(&intent, || Ok(open()))
        .expect("issued before the poison");
    let lineage = issued.lineage().to_vec();
    let resolved = owner
        .resolve_controller_lineage(&lineage, &who("helper"), None)
        .expect("usable before the poison");
    assert_eq!(resolved.check_current(), Ok(()));

    owner.publication.poison();
    assert_eq!(resolved.check_current(), Err(PublicationError::Unavailable));
    assert!(matches!(
        owner.resolve_controller_lineage(&lineage, &who("helper"), None),
        Err(GrantRefusal::Unavailable)
    ));

    Arc::get_mut(&mut owner)
        .expect("the only reference to the owner")
        .reconstruct(LocalAuthorizationPublication::new())
        .expect("reconstructed under a new publication");
    assert_eq!(
        owner.issue_or_recover_child_under_exclusion(&intent, || Ok(open())),
        Ok(ChildGrantOutcome::Recovered { lineage })
    );
}

/// Blocks the first clock read after the issuance's conjunct arms it: the
/// final read before the append, which follows the pre-append check.
struct ArmedGateClock {
    armed: AtomicBool,
    gate: Mutex<Option<(SyncSender<()>, Receiver<()>)>>,
}
impl LocalAuthorizationClock for ArmedGateClock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        if self.armed.load(Ordering::SeqCst) {
            let gate = self.gate.lock().expect("test gate").take();
            if let Some((entered, release)) = gate {
                entered
                    .send(())
                    .expect("the issuance reached its final clock read");
                release
                    .recv_timeout(Duration::from_secs(5))
                    .expect("release the issuance");
            }
        }
        Ok(now())
    }
}

/// A poison another thread raises after the pre-append check cannot stop the
/// append that follows: the child is issued, stays unusable while the
/// publication is poisoned, and the same intent recovers it after
/// reconstruction.
#[test]
fn a_poison_after_the_pre_append_check_leaves_the_issued_child_unusable_until_reconstruction() {
    let (entered_tx, entered_rx) = sync_channel(1);
    let (release_tx, release_rx) = sync_channel(1);
    let clock = Arc::new(ArmedGateClock {
        armed: AtomicBool::new(false),
        gate: Mutex::new(Some((entered_tx, release_rx))),
    });
    let mut owner = authority(clock.clone());
    let parent = issue(&owner, "parent");
    let before = stamp(&owner);
    let intent = child_intent(&parent, "child");
    let issuing_owner = Arc::clone(&owner);
    let issuing_intent = intent.clone();
    let issuer = std::thread::spawn(move || {
        issuing_owner.issue_or_recover_child_under_exclusion(&issuing_intent, || {
            // The next clock read is the final one before the append.
            clock.armed.store(true, Ordering::SeqCst);
            Ok(open())
        })
    });
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the issuance passed its pre-append check");
    owner.publication.poison();
    release_tx.send(()).expect("release the issuance");
    let issued = issuer
        .join()
        .expect("issuing thread")
        .expect("an append past its pre-append check lands");
    assert!(
        matches!(issued, ChildGrantOutcome::Issued { .. }),
        "{issued:?}"
    );
    let lineage = issued.lineage().to_vec();
    assert_eq!(
        before.check_current(),
        Err(PublicationError::Unavailable),
        "the append's publication does not overwrite the poison"
    );
    assert!(matches!(
        owner.publication.observe(|| ()),
        Err(PublicationError::Unavailable)
    ));
    assert!(matches!(
        owner.resolve_controller_lineage(&lineage, &who("helper"), None),
        Err(GrantRefusal::Unavailable)
    ));
    assert_eq!(
        owner.issue_or_recover_child_under_exclusion(&intent, || Ok(open())),
        Err(ChildGrantRefusal::Unavailable)
    );

    Arc::get_mut(&mut owner)
        .expect("the only reference to the owner")
        .reconstruct(LocalAuthorizationPublication::new())
        .expect("reconstructed under a new publication");
    assert_eq!(
        owner.issue_or_recover_child_under_exclusion(&intent, || Ok(open())),
        Ok(ChildGrantOutcome::Recovered {
            lineage: lineage.clone()
        })
    );
    owner
        .resolve_controller_lineage(&lineage, &who("helper"), None)
        .expect("the recovered child is valid after reconstruction");
}
