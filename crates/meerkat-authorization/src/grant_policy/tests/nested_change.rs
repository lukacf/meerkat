//! A grant resolution inside a preparation that meets an owner change in
//! progress reads stale facts: the preparation classifies that as a stale
//! refusal and rebuilds once, never as a known infrastructure failure. A
//! poison or a clock failure in the same place stays unavailable.

use super::*;
use meerkat_core::OperationAuthorizationError;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

/// What the work owner does on its first call, inside the preparation's
/// observation and before the grant is resolved.
enum Interference {
    /// A helper thread begins an owner change on the shared publication and
    /// holds it until the test releases it.
    HoldOwnerChange {
        begin: SyncSender<()>,
        held: Receiver<()>,
    },
    Poison(LocalAuthorizationPublication),
    FailClock(Arc<Clock>),
}

/// The fixture's admitted-work owner, interfering once.
struct Interfering {
    inner: Arc<Admitted>,
    pending: Mutex<Option<Interference>>,
}

impl AdmittedWorkPolicyOwner for Interfering {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        let interference = self.pending.lock().expect("interference").take();
        match interference {
            Some(Interference::HoldOwnerChange { begin, held }) => {
                begin.send(()).expect("start the owner change");
                held.recv_timeout(Duration::from_secs(5))
                    .expect("the owner change is in progress");
            }
            Some(Interference::Poison(publication)) => publication.poison(),
            Some(Interference::FailClock(clock)) => clock.1.store(true, Ordering::SeqCst),
            None => {}
        }
        self.inner
            .authorize_admitted_work(association, binding, purpose, now_ms)
    }
}

fn interfering_context(
    fixture: &Fixture,
    interference: Interference,
) -> (WorkAuthorizationContext, PreparedAuthorizationBinding) {
    let association = fixture.association();
    let (_, admitted, resources) = fixture.policy(association.clone());
    let owner = Arc::new(Interfering {
        inner: admitted,
        pending: Mutex::new(Some(interference)),
    });
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        fixture.grants.clone(),
        owner,
        resources,
    ));
    (
        fixture.context(&policy, &association),
        source(SourceAuthorizationUse::Read, "public"),
    )
}

#[test]
fn an_owner_change_in_progress_during_grant_resolution_is_a_stale_refusal() {
    let fixture = Fixture::new();
    let (begin_tx, begin_rx) = sync_channel(1);
    let (held_tx, held_rx) = sync_channel(1);
    let (release_tx, release_rx) = sync_channel::<()>(1);
    let (context, binding) = interfering_context(
        &fixture,
        Interference::HoldOwnerChange {
            begin: begin_tx,
            held: held_rx,
        },
    );
    let publication = fixture.publication;
    let helper = std::thread::spawn(move || {
        begin_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("asked to change an owner");
        let change = publication
            .begin_owner_change()
            .expect("an owner change in progress");
        held_tx.send(()).expect("the change is held");
        release_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("released by the test");
        drop(change);
    });
    let refusal = context.authorization().prepare(&binding).err();
    release_tx.send(()).expect("release the owner change");
    helper.join().expect("helper thread");
    assert!(
        matches!(
            &refusal,
            Some(OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::ReprepareRequired
        ),
        "a change in progress is a stale read, rebuilt once, never unavailable: {refusal:?}"
    );
    context
        .authorization()
        .prepare(&binding)
        .expect("a fresh preparation succeeds once the change completes");
}

#[test]
fn a_poison_during_grant_resolution_stays_unavailable() {
    let fixture = Fixture::new();
    let (context, binding) =
        interfering_context(&fixture, Interference::Poison(fixture.publication.clone()));
    let refusal = context.authorization().prepare(&binding).err();
    assert!(
        matches!(refusal, Some(OperationAuthorizationError::Unavailable)),
        "{refusal:?}"
    );
}

#[test]
fn a_clock_failure_during_grant_resolution_stays_unavailable() {
    let fixture = Fixture::new();
    let (context, binding) =
        interfering_context(&fixture, Interference::FailClock(fixture.clock.clone()));
    let refusal = context.authorization().prepare(&binding).err();
    assert!(
        matches!(refusal, Some(OperationAuthorizationError::Unavailable)),
        "{refusal:?}"
    );
}
