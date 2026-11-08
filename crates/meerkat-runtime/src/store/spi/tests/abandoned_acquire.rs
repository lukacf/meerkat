//! H0: an acquire the realm gave up on, which the store grants anyway.
//!
//! A stopped writer re-acquires its fence, and each acquire of the
//! re-acquire runs under the realm's acquire timeout. A store that completes
//! an acquire after the realm dropped it leaves a token the writer never
//! recorded, and the writer's next re-acquire meets that token as the
//! holder. While an acquire of its own timed out since its last grant, a
//! writer cannot tell such a holder from another writer: it waits out the
//! holder's recorded lease and acquires again. Only a holder that renews its
//! lease (a live writer) or a batch the writer did not write poisons the
//! realm. With no timed-out acquire, an unknown holder is another writer at
//! once (`a_writer_that_stopped_never_resumes_on_a_later_renew`).
//!
//! A grant the writer did get is its own as well while its token is not
//! installed: a re-acquire installs the token only once its listing showed
//! no batch it did not write, and a listing that fails leaves the store
//! holding the grant's lease.

use super::*;

/// A 1 s renew interval on a 3 s lease (a 2 s self-fence window), whose
/// realm gives each acquire of a re-acquire 1 s.
async fn open_fast(host: &Arc<TestHost>, acquire: AcquireMode) -> (Arc<SpiRealm>, SpiRuntimeStore) {
    let config = SpiRealmConfig {
        acquire,
        renew_timeout: Duration::from_secs(1),
        ..SpiRealmConfig::default()
    };
    let realm = SpiRealm::open(Arc::clone(host) as _, config)
        .await
        .expect("realm opens");
    let store = SpiRuntimeStore::open(Arc::clone(&realm))
        .await
        .expect("store opens");
    (realm, store)
}

fn fast_host() -> Arc<TestHost> {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    Arc::new(TestHost::with_timing(timing))
}

// Unrun until the candidate's validation. A renew stop at 1 s; at 2 s the
// re-acquire's exclusive acquire answers `Held` with the writer's own token,
// and the takeover of its own lease never answers: the host grants it on its
// own at 2.5 s (token 2, its lease until 5.5 s), and the realm gave up on it
// at 3 s. The next re-acquire (3 s) meets token 2 as the holder. The writer
// may be meeting its own grant, so it waits out token 2's recorded lease,
// acquires again once that lease ended (token 3 at 6 s), finds no batch it
// did not write and resumes. Before, it poisoned the realm at the first
// `Held`.
#[tokio::test(start_paused = true)]
async fn a_reacquire_that_meets_its_own_late_takeover_waits_out_its_lease_and_resumes() {
    let host = fast_host();
    let (realm, store) = open_fast(&host, AcquireMode::Exclusive).await;
    assert_eq!(realm.fence().get(), 1);
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.acquire_granted_late = Some((AcquireMode::Takeover, Duration::from_millis(500)));
    }
    tokio::time::sleep(Duration::from_millis(4500)).await;
    assert_eq!(
        host.token(),
        2,
        "the host granted the takeover after the realm gave up"
    );
    assert!(
        realm.realm_poisoned().is_none(),
        "a holder that may be the writer's own late grant is waited out, not taken for another \
         writer: {:?}",
        realm.realm_poisoned()
    );
    assert!(!realm.lease_is_live(), "the writer waits, stopped");
    assert_eq!(realm.fence().get(), 1, "the writer never recorded token 2");

    // Token 2's recorded lease ended at 5.5 s; the tick at 6 s acquires.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "{:?}",
        realm.realm_poisoned()
    );
    assert!(realm.lease_is_live(), "the writer resumed");
    assert_eq!(
        realm.fence().get(),
        3,
        "under a fresh token above the late grant"
    );
    assert_eq!(host.token(), 3);
    store
        .persist_input_state(
            &LogicalRuntimeId::new("resumed"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("writes resume under token 3");
}

// Unrun until the candidate's validation. The writer's exclusive acquire at
// 2 s times out at 3 s and grants nothing; meanwhile another writer took the
// fence over at 2.5 s (token 2) and renews it every second. The writer
// cannot tell token 2 from a late grant of its own acquire, so it waits out
// token 2's recorded lease (until 5.5 s). At 6 s token 2 still holds the
// fence: it renewed, and only a live writer renews, so the realm is
// poisoned. The writer never wrote again, and the other writer keeps
// serving. Before, the writer poisoned the realm at its first `Held`
// (3 s), with no wait.
#[tokio::test(start_paused = true)]
async fn a_waited_out_holder_that_renews_its_lease_poisons_the_realm() {
    let host = fast_host();
    let (realm, store) = open_fast(&host, AcquireMode::Exclusive).await;
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.acquire_hang = 1;
    }
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let (other_realm, other_store) = open_fast(&host, AcquireMode::Takeover).await;
    assert_eq!(other_realm.fence().get(), 2);
    let landed = host.commit_count();

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "the writer waits out token 2's recorded lease before it judges it: {:?}",
        realm.realm_poisoned()
    );
    assert!(!realm.lease_is_live());

    tokio::time::sleep(Duration::from_secs(3)).await;
    let cause = realm
        .realm_poisoned()
        .expect("a holder that renewed past its recorded lease is another live writer");
    assert!(cause.contains("renewed"), "{cause}");
    let refused = store
        .persist_input_state(
            &LogicalRuntimeId::new("late"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await;
    assert!(
        matches!(refused, Err(RuntimeStoreError::RealmRefused { .. })),
        "{refused:?}"
    );
    assert_eq!(host.commit_count(), landed, "the writer never wrote again");
    other_store
        .persist_input_state(
            &LogicalRuntimeId::new("other"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("the other writer keeps serving");
    assert!(other_realm.realm_poisoned().is_none());
}

// Unrun until the candidate's validation. As above, but the other writer
// commits a batch at 3 s and stops: its lease is not renewed and ends at
// 5.5 s. The writer waits it out, acquires at 6 s (token 3), and its listing
// shows the other writer's batch: the realm is poisoned there, and the
// writer never serves the other writer's scope. Before, it poisoned the realm
// at its first `Held` (3 s), as a holder it did not know.
#[tokio::test(start_paused = true)]
async fn a_waited_out_holder_that_committed_poisons_the_realm_at_the_listing() {
    let host = fast_host();
    let (realm, store) = open_fast(&host, AcquireMode::Exclusive).await;
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.acquire_hang = 1;
    }
    tokio::time::sleep(Duration::from_millis(2500)).await;
    {
        let (other_realm, other_store) = open_fast(&host, AcquireMode::Takeover).await;
        assert_eq!(other_realm.fence().get(), 2);
        other_store
            .persist_input_state(
                &LogicalRuntimeId::new("foreign"),
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .expect("the other writer commits");
    }

    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "the writer waits out token 2's recorded lease before it judges it: {:?}",
        realm.realm_poisoned()
    );

    tokio::time::sleep(Duration::from_secs(3)).await;
    let cause = realm
        .realm_poisoned()
        .expect("the listing after the wait shows the other writer's batch");
    assert!(cause.contains("another writer committed"), "{cause}");
    assert_eq!(
        host.token(),
        3,
        "the writer acquired once token 2's lease ended"
    );
    let refused = store
        .load_input_states(&LogicalRuntimeId::new("foreign"))
        .await;
    assert!(
        matches!(refused, Err(RuntimeStoreError::RealmRefused { .. })),
        "{refused:?}"
    );
}

// Unrun until the candidate's validation. A renew stop at 1 s; at 2 s the
// re-acquire's exclusive acquire answers `Held` with the writer's own token,
// and the takeover of its own lease is granted (token 2, its lease until
// 5 s), but the listing that would show no batch the writer did not write
// fails. The writer stays stopped under token 1. At 3 s the next re-acquire
// meets token 2 as the holder: the writer's own grant, whose lease it takes
// over (token 3); the listing passes and the writer resumes. Before, the
// writer forgot token 2 when the listing failed, met it at 3 s as a holder it
// did not know, with no timed-out acquire, and poisoned the realm.
#[tokio::test(start_paused = true)]
async fn a_grant_whose_listing_failed_is_taken_over_as_the_writers_own() {
    let host = fast_host();
    let (realm, store) = open_fast(&host, AcquireMode::Exclusive).await;
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.list_unavailable = 1;
    }
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(host.token(), 2, "the re-acquire was granted token 2");
    assert!(
        realm.realm_poisoned().is_none(),
        "{:?}",
        realm.realm_poisoned()
    );
    assert!(
        !realm.lease_is_live(),
        "its listing failed: the writer stays stopped"
    );
    assert_eq!(
        realm.fence().get(),
        1,
        "the writer commits with no token its listing did not prove"
    );

    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "a grant of the writer's own whose listing failed is not another writer: {:?}",
        realm.realm_poisoned()
    );
    assert!(realm.lease_is_live(), "the writer resumed");
    assert_eq!(
        realm.fence().get(),
        3,
        "on a takeover of its own grant's lease"
    );
    assert_eq!(host.token(), 3);
    store
        .persist_input_state(
            &LogicalRuntimeId::new("resumed"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("writes resume under token 3");
}

// A renew stop at 1 s; the re-acquire's exclusive acquire at 2 s never
// answers and times out at 3 s; the tick at 3 s acquires token 2 (token 1's
// lease ended) and the writer resumes. A grant clears the count of acquires
// that timed out: once the writer stops again and another writer opens with
// token 3, the next re-acquire meets token 3 as the holder and loses the
// realm at once, as a writer with no timed-out acquire does. Had the grant
// kept the count, the writer would wait out another writer's lease instead.
#[tokio::test(start_paused = true)]
async fn a_grant_clears_the_count_of_timed_out_acquires() {
    let host = fast_host();
    let (realm, _store) = open_fast(&host, AcquireMode::Exclusive).await;
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.acquire_hang = 1;
    }
    tokio::time::sleep(Duration::from_millis(4500)).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "{:?}",
        realm.realm_poisoned()
    );
    assert!(realm.lease_is_live(), "the writer resumed");
    assert_eq!(
        realm.fence().get(),
        2,
        "granted after the timed-out acquire"
    );
    host.faults.lock().unwrap().renew_unavailable = 1;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!realm.lease_is_live(), "stopped again");
    host.expire_lease();
    let (other_realm, _other_store) = open_fast(&host, AcquireMode::Exclusive).await;
    assert_eq!(other_realm.fence().get(), 3);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let cause = realm
        .realm_poisoned()
        .expect("with no acquire timed out since the last grant, an unknown holder poisons");
    assert!(cause.contains("token 3 holds the realm"), "{cause}");
}

// As `a_reacquire_that_meets_its_own_late_takeover_waits_out_its_lease_and_resumes`
// (the writer's own takeover granted late: token 2, its lease until 5.5 s),
// through a store that answers `Held` with its remaining lease rounded down
// to whole seconds. At 3 s the re-acquire meets token 2 with 2.5 s left,
// answered as 2 s, so the writer's recorded end (5 s) comes before the
// store's own. The acquire at 5 s meets token 2 again, with 0.5 s left: its
// recorded end did not move, so nobody renewed it, and the writer waits out
// the rest and resumes on token 3. Before, meeting the same holder after its
// recorded end lost the realm as if token 2 had renewed its lease.
#[tokio::test(start_paused = true)]
async fn a_rounded_down_remaining_lease_does_not_poison_the_writers_own_late_grant() {
    let host = fast_host();
    let (realm, _store) = open_fast(&host, AcquireMode::Exclusive).await;
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.acquire_granted_late = Some((AcquireMode::Takeover, Duration::from_millis(500)));
        faults.held_remaining_floor = Some(Duration::from_secs(1));
    }
    tokio::time::sleep(Duration::from_millis(7500)).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "a holder nobody renewed is not another writer: {:?}",
        realm.realm_poisoned()
    );
    assert!(realm.lease_is_live(), "the writer resumed");
    assert_eq!(
        realm.fence().get(),
        3,
        "on a fresh grant after token 2's lease"
    );
}

// A renew stop at 1 s; the exclusive acquire at 2 s never answers and times
// out at 3 s; another writer takes the fence over at 2.5 s (token 2, its
// lease until 5.5 s) and is gone at once: nobody renews token 2. The store
// answers `Held` with its remaining lease rounded down to whole multiples of
// 3 s, so the re-acquire at 3 s meets token 2 with 0 s left and the writer's
// recorded end is 3 s. From then on every acquire answers 0.5 s after the
// store decided it, as over a slow network: at 4 s the store still names
// token 2 with 0 s left, an answer that arrives at 4.5 s. The lease did not
// move, so the writer waits again, acquires token 3 once token 2's lease
// ended and resumes. Before, the writer read the end as the answer's arrival
// plus its remaining lease (4.5 s), past its recorded end plus one renew
// interval (4 s), took token 2 for a holder that renewed, and lost the realm.
#[tokio::test(start_paused = true)]
async fn a_slow_answer_does_not_read_an_unrenewed_holder_as_renewed() {
    let host = fast_host();
    let (realm, _store) = open_fast(&host, AcquireMode::Exclusive).await;
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.acquire_hang = 1;
        faults.held_remaining_floor = Some(Duration::from_secs(3));
    }
    tokio::time::sleep(Duration::from_millis(2500)).await;
    {
        let (other_realm, _other_store) = open_fast(&host, AcquireMode::Takeover).await;
        assert_eq!(other_realm.fence().get(), 2);
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "the writer waits out token 2: {:?}",
        realm.realm_poisoned()
    );
    host.faults.lock().unwrap().acquire_delay = Some(Duration::from_millis(500));

    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        realm.realm_poisoned().is_none(),
        "a slow answer naming a holder nobody renewed is not a renewed lease: {:?}",
        realm.realm_poisoned()
    );
    assert!(realm.lease_is_live(), "the writer resumed");
    assert_eq!(
        realm.fence().get(),
        3,
        "on a fresh grant after token 2's lease"
    );
}

// A renew stop at 1 s; the exclusive acquire at 2 s never answers and times
// out at 3 s; another writer takes the fence over at 2.5 s (token 2). The
// re-acquire then meets token 2 through a store that answers `Held` with a
// remaining lease beyond any lease (outside the contract). The writer caps
// it at its declared lease and waits; it does not panic. Before, the
// recorded end overflowed the clock, the re-acquire task panicked, and the
// realm neither renewed nor recovered.
#[tokio::test(start_paused = true)]
async fn an_out_of_contract_remaining_lease_does_not_panic_the_reacquire() {
    let host = fast_host();
    let (realm, _store) = open_fast(&host, AcquireMode::Exclusive).await;
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1;
        faults.acquire_hang = 1;
        faults.held_remaining_override = Some(Duration::MAX);
    }
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let (_other_realm, _other_store) = open_fast(&host, AcquireMode::Takeover).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let answer = realm.ensure_lease().await;
    let text = format!("{answer:?}");
    assert!(
        !text.contains("panicked"),
        "the re-acquire task panicked: {text}"
    );
    assert!(
        matches!(answer, Err(SpiError::LeaseLapsed(_))),
        "the writer waits out the holder's capped lease: {answer:?}"
    );
}
