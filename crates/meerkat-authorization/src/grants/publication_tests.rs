//! C5 regression tests: authored before the isolated production patch.
#![allow(clippy::expect_used)]

use super::*;
use crate::clock::{LocalAuthorizationTime, LocalClockError};
use crate::publication::{LocalPublicationStamp, PublicationError};
use meerkat_core::time_compat::Instant;
use meerkat_core::{PrincipalKind, TrustDomainId};
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
