//! Cold owner reconstruction, not decoded-state admission or runtime recovery.
#![allow(clippy::expect_used, clippy::panic)]

use super::*;
use crate::clock::{LocalAuthorizationTime, LocalClockError};
use crate::publication::PublicationError;
use meerkat_authorization_contracts::constraints::{LifetimeRestriction, UnresolvedConstraint};
use meerkat_core::time_compat::Instant;
use meerkat_core::{PrincipalKind, TrustDomainId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::sync_channel;
use std::time::Duration;

struct Clock(AtomicBool);
impl LocalAuthorizationClock for Clock {
    fn now(&self) -> Result<LocalAuthorizationTime, LocalClockError> {
        assert!(
            !self.0.swap(false, Ordering::SeqCst),
            "one actual clock panic"
        );
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
        TrustDomainId::new("reconstruction").expect("domain"),
    )
    .expect("principal")
}
fn id(name: &str) -> EvidenceId {
    EvidenceId::new(name).expect("id")
}
fn fixture() -> (LocalGrantAuthority, Arc<Clock>) {
    let clock = Arc::new(Clock(AtomicBool::new(false)));
    let owner = LocalGrantAuthority::new(
        LocalGrantConfiguration {
            root: who("root"),
            namespace: id("namespace"),
            generation: 1,
        },
        LocalAuthorizationPublication::new(),
        clock.clone(),
    )
    .expect("configure");
    (owner, clock)
}
fn issue(
    owner: &LocalGrantAuthority,
    name: &str,
    restrictions: ExecutionRestrictions,
) -> GrantLineageRef {
    owner
        .issue_root(&who("root"), id(name), who("holder"), None, restrictions)
        .expect("issue")
}
fn poison_only_owner(owner: &LocalGrantAuthority) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _state = owner.owner.lock().expect("actual owner lock");
        panic!("owner-only panic; old publication is initially healthy");
    }));
    assert!(result.is_err());
}

#[test]
fn reconstruction_preserves_revocations_and_retires_healthy_old_publication() {
    let (mut owner, _) = fixture();
    let allowed = issue(&owner, "allowed", ExecutionRestrictions::unrestricted());
    let revoked = issue(&owner, "revoked", ExecutionRestrictions::unrestricted());
    owner
        .revoke(&who("root"), &revoked, &mut IsolatedGrantTestCustody)
        .expect("revoke");
    let before = owner.owner.lock().expect("owner").state().clone();
    let old = owner.publication.clone();
    let resolved = owner
        .resolve_lineage(std::slice::from_ref(&allowed), &who("holder"), None)
        .expect("before");
    poison_only_owner(&owner);
    assert_eq!(
        resolved.check_current(),
        Ok(()),
        "owner poison alone has no publication hook"
    );
    owner
        .reconstruct(LocalAuthorizationPublication::new())
        .expect("verified actual owner");
    assert_eq!(owner.owner.lock().expect("new mutex").state(), &before);
    assert_eq!(resolved.check_current(), Err(PublicationError::Unavailable));
    assert!(matches!(
        old.begin_owner_change(),
        Err(PublicationError::Unavailable)
    ));
    let fresh = owner
        .resolve_lineage(std::slice::from_ref(&allowed), &who("holder"), None)
        .expect("permitted after reconstruction");
    assert_eq!(fresh.check_current(), Ok(()));
    assert!(matches!(
        owner.resolve_lineage(&[revoked], &who("holder"), None),
        Err(GrantRefusal::Denied)
    ));
}

#[test]
fn actual_prewrite_panic_reconstructs_without_replaying_the_failed_child() {
    let (mut owner, clock) = fixture();
    let parent = issue(&owner, "parent", ExecutionRestrictions::unrestricted());
    let before = owner.owner.lock().expect("owner").state().clone();
    let ((), old_stamp) = owner.publication.observe(|| ()).expect("stamp");
    clock.0.store(true, Ordering::SeqCst);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = owner.issue_child(
            &who("holder"),
            &parent,
            id("unissued"),
            who("child"),
            ExecutionRestrictions::unrestricted(),
        );
    }));
    assert!(result.is_err());
    assert_eq!(
        old_stamp.check_current(),
        Err(PublicationError::Unavailable)
    );
    owner
        .reconstruct(LocalAuthorizationPublication::new())
        .expect("unchanged valid actual owner");
    assert_eq!(owner.owner.lock().expect("owner").state(), &before);
    owner
        .resolve_lineage(&[parent], &who("holder"), None)
        .expect("old reference retains actual incarnation");
    assert_eq!(
        old_stamp.check_current(),
        Err(PublicationError::Unavailable)
    );
}

#[test]
fn reconstruction_preserves_a_real_narrowed_three_level_chain() {
    use meerkat_authorization_contracts::constraints::{
        ActionRef, DelegationDepth, ExactRestriction,
    };
    let (mut owner, _) = fixture();
    let root = issue(
        &owner,
        "root-grant",
        ExecutionRestrictions {
            actions: ExactRestriction::exact([ActionRef {
                feature: "files".into(),
                action: "read".into(),
            }]),
            lifetime: LifetimeRestriction::window(1, 200),
            delegation_depth: DelegationDepth::remaining(2),
            ..ExecutionRestrictions::unrestricted()
        },
    );
    let child = owner
        .issue_child(
            &who("holder"),
            &root,
            id("child"),
            who("child"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("first actual narrowing");
    let leaf = owner
        .issue_child(
            &who("child"),
            &child,
            id("leaf"),
            who("leaf"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("second actual narrowing");
    let before = owner.owner.lock().expect("owner").state().clone();
    let references = [root, child, leaf];
    let previous = owner
        .resolve_lineage(&references, &who("leaf"), None)
        .expect("original exact chain");
    poison_only_owner(&owner);
    owner
        .reconstruct(LocalAuthorizationPublication::new())
        .expect("existing narrowing is a fixed point");
    let current = owner
        .resolve_lineage(&references, &who("leaf"), None)
        .expect("same exact chain after verified reconstruction");
    assert_eq!(current.restrictions(), previous.restrictions());
    assert_eq!(current.expires_at_ms(), 200);
    assert_eq!(owner.owner.lock().expect("owner").state(), &before);
    assert_eq!(previous.check_current(), Err(PublicationError::Unavailable));
}

#[test]
fn failed_reconstruction_keeps_the_actual_unverifiable_owner_and_revocation() {
    let (mut owner, _) = fixture();
    let parent = issue(&owner, "parent", ExecutionRestrictions::unrestricted());
    let child = owner
        .issue_child(
            &who("holder"),
            &parent,
            id("child"),
            who("child"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("child");
    let revoked = issue(&owner, "revoked", ExecutionRestrictions::unrestricted());
    owner
        .revoke(&who("root"), &revoked, &mut IsolatedGrantTestCustody)
        .expect("revoke");
    let mut corrupt = owner.owner.lock().expect("owner").state().clone();
    // Test-only retained-state corruption: shape is valid but the immutable
    // parent/child subject relation is not. No public reconstruction data API.
    corrupt
        .records
        .get_mut(&child.grant_id)
        .expect("child")
        .represented_subject = Some(principal(who("substituted")).expect("principal"));
    owner.owner = Mutex::new(
        GrantAuthorityMachineAuthority::recover_from_state(corrupt.clone())
            .expect("shape checks alone are not lineage validation"),
    );
    poison_only_owner(&owner);
    let replacement = LocalAuthorizationPublication::new();
    let ((), replacement_before) = replacement.observe(|| ()).expect("replacement stamp");
    assert_eq!(
        owner.reconstruct(replacement),
        Err(GrantReconstructionError::RestartRequired)
    );
    let retained = match owner.owner.lock() {
        Err(poisoned) => poisoned.into_inner(),
        Ok(_) => panic!("failed reconstruction must not clear owner poison"),
    };
    assert_eq!(retained.state(), &corrupt);
    assert!(retained.state().revoked.contains(&revoked.grant_id));
    assert_eq!(
        replacement_before.check_current(),
        Ok(()),
        "refused reconstruction publishes no new facts"
    );
}

#[test]
fn reconstruction_rejects_a_widened_retained_child() {
    use meerkat_authorization_contracts::constraints::{ActionRef, ExactRestriction};
    let (mut owner, _) = fixture();
    let parent = issue(
        &owner,
        "parent",
        ExecutionRestrictions {
            actions: ExactRestriction::exact([ActionRef {
                feature: "files".into(),
                action: "read".into(),
            }]),
            ..ExecutionRestrictions::unrestricted()
        },
    );
    let child = owner
        .issue_child(
            &who("holder"),
            &parent,
            id("child"),
            who("child"),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("narrowed child");
    let mut corrupt = owner.owner.lock().expect("owner").state().clone();
    corrupt
        .records
        .get_mut(&child.grant_id)
        .expect("child")
        .restrictions = ExecutionRestrictions::unrestricted();
    owner.owner = Mutex::new(
        GrantAuthorityMachineAuthority::recover_from_state(corrupt.clone())
            .expect("shape alone cannot prove attenuation"),
    );
    poison_only_owner(&owner);
    assert_eq!(
        owner.reconstruct(LocalAuthorizationPublication::new()),
        Err(GrantReconstructionError::RestartRequired)
    );
    let retained = match owner.owner.lock() {
        Err(poisoned) => poisoned.into_inner(),
        Ok(_) => panic!("failed reconstruction must not clear owner poison"),
    };
    assert_eq!(retained.state(), &corrupt);
}

#[test]
fn replacement_must_be_different_and_available_without_reviving_old_stamps() {
    let (mut owner, _) = fixture();
    let _ = issue(&owner, "retained", ExecutionRestrictions::unrestricted());
    let before = owner.owner.lock().expect("owner").state().clone();
    let old = owner.publication.clone();
    let ((), old_stamp) = old.observe(|| ()).expect("stamp");
    assert_eq!(
        owner.reconstruct(old),
        Err(GrantReconstructionError::ReplacementPublicationUnavailable)
    );
    assert_eq!(
        old_stamp.check_current(),
        Err(PublicationError::Unavailable)
    );
    let bad = LocalAuthorizationPublication::new();
    bad.retire();
    assert_eq!(
        owner.reconstruct(bad),
        Err(GrantReconstructionError::ReplacementPublicationUnavailable)
    );
    assert_eq!(
        owner.owner.lock().expect("unchanged owner").state(),
        &before
    );
    owner
        .reconstruct(LocalAuthorizationPublication::new())
        .expect("fresh retry verifies same retained owner");
    assert_eq!(owner.owner.lock().expect("owner").state(), &before);
    assert_eq!(
        old_stamp.check_current(),
        Err(PublicationError::Unavailable)
    );
}

#[test]
fn retirement_waits_for_writer_drop_and_cannot_be_overwritten_afterwards() {
    let old = LocalAuthorizationPublication::new();
    let ((), stamp) = old.observe(|| ()).expect("stamp");
    let writer = old.begin_owner_change().expect("in-flight actual writer");
    let retire = old.clone();
    let (started_tx, started_rx) = sync_channel(1);
    let (done_tx, done_rx) = sync_channel(1);
    let thread = std::thread::spawn(move || {
        started_tx.send(()).expect("start");
        retire.retire();
        done_tx.send(()).expect("retired");
    });
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("retirement thread reached call");
    assert!(
        matches!(
            done_rx.recv_timeout(Duration::from_millis(50)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ),
        "retirement cannot pass an owned writer guard"
    );
    drop(writer);
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("retired after writer drop");
    thread.join().expect("retirement thread");
    assert_eq!(stamp.check_current(), Err(PublicationError::Unavailable));
    assert!(matches!(
        old.begin_owner_change(),
        Err(PublicationError::Unavailable)
    ));
    assert!(matches!(
        old.observe(|| ()),
        Err(PublicationError::Unavailable)
    ));
}

#[test]
fn expired_and_unknown_retained_rows_are_preserved_not_reauthorized() {
    let (mut owner, _) = fixture();
    let allowed = issue(&owner, "allowed", ExecutionRestrictions::unrestricted());
    let expired = issue(
        &owner,
        "expired",
        ExecutionRestrictions {
            lifetime: LifetimeRestriction::window(1, 2),
            ..ExecutionRestrictions::unrestricted()
        },
    );
    let unknown = issue(
        &owner,
        "unknown",
        ExecutionRestrictions {
            lifetime: LifetimeRestriction::unresolved(UnresolvedConstraint::Unknown),
            ..ExecutionRestrictions::unrestricted()
        },
    );
    let before = owner.owner.lock().expect("owner").state().clone();
    poison_only_owner(&owner);
    owner
        .reconstruct(LocalAuthorizationPublication::new())
        .expect("retain restrictive history");
    assert_eq!(owner.owner.lock().expect("owner").state(), &before);
    owner
        .resolve_lineage(&[allowed], &who("holder"), None)
        .expect("permitted independent grant");
    assert!(matches!(
        owner.resolve_lineage(&[expired], &who("holder"), None),
        Err(GrantRefusal::Denied)
    ));
    assert!(matches!(
        owner.resolve_lineage(&[unknown], &who("holder"), None),
        Err(GrantRefusal::Denied)
    ));
}
