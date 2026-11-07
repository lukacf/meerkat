//! The open's listing and the writer's fence. A realm installs what its open
//! listed (the scope versions it knows) only once that listing ends, and its
//! writer's renewal runs from the open on: a re-acquire may compare a
//! listing only with a baseline the open installed, so it never reads the
//! realm's own scopes as another writer's, and the realm of an open that
//! failed before installing one never re-acquires.

use super::*;

// A store whose listing is slow (3.5 s, longer than its 1 s renew interval)
// and whose first renew fails, met at the open of a realm that holds a scope:
// the renew at 1 s fails and stops the writer while the open still lists, and
// the tick at 2 s re-acquires. No other writer exists, so the realm must not
// be poisoned, and once the open's listing ended the writer serves. Before,
// the re-acquire compared its listing with the baseline the open had not
// installed yet (empty), read the realm's own scope as another writer's
// batch, and poisoned the realm ("another writer committed while this one
// was stopped"). Real time: before 0.9.11 the realm read the system clock,
// which a paused test clock does not move.
#[tokio::test]
async fn a_renew_failure_during_the_opens_listing_poisons_nothing() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let opened = open(&host).await;
    opened
        .1
        .persist_input_state(
            &LogicalRuntimeId::new("held"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("the runtime writes");
    drop(opened);
    host.expire_lease();
    {
        let mut faults = host.faults.lock().unwrap();
        faults.list_delay = Some(Duration::from_millis(3500));
        faults.renew_unavailable = 1;
    }
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("the realm opens");
    assert!(
        realm.realm_poisoned().is_none(),
        "a renew failure during the open's listing poisons nothing: {:?}",
        realm.realm_poisoned()
    );
    // The writer re-acquires once the open's listing is in place, and serves.
    let started = std::time::Instant::now();
    while !realm.lease_is_live() && started.elapsed() < Duration::from_secs(10) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(realm.lease_is_live(), "the writer serves after the open");
    assert!(
        realm.realm_poisoned().is_none(),
        "{:?}",
        realm.realm_poisoned()
    );
    let store = SpiRuntimeStore::open(Arc::clone(&realm))
        .await
        .expect("the runtime store opens");
    assert_eq!(
        store
            .load_input_states(&LogicalRuntimeId::new("held"))
            .await
            .expect("the runtime is served")
            .len(),
        1
    );
}

// A slow open whose listing then fails: the open answers the failure and its
// realm is never served. The realm holds two session scopes and the store
// answers one head a page, so the open lists the session kind in two pages:
// the first waits 2.5 s and the second fails. The re-acquire the failed
// renew queued waits for the open's listing, finds no baseline installed,
// and neither acquires nor lists. Before, it acquired the fence for a realm
// nobody holds (an exclusive acquire, then the takeover of its own lease).
#[tokio::test]
async fn an_open_whose_listing_fails_leaves_no_acquire_behind() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let opened = open(&host).await;
    for name in ["first", "second"] {
        opened
            .1
            .persist_input_state(
                &LogicalRuntimeId::new(name),
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .expect("the runtime writes");
    }
    drop(opened);
    host.expire_lease();
    host.take_calls();
    {
        let mut faults = host.faults.lock().unwrap();
        faults.list_page_cap = Some(1);
        faults.list_delay = Some(Duration::from_millis(2500));
        faults.renew_unavailable = 1;
    }
    let opening = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default());
    let fail_the_next_listing_call = async {
        // The renew at 1 s failed and the tick at 2 s queued a re-acquire,
        // while the open's first session page still waits.
        tokio::time::sleep(Duration::from_millis(2200)).await;
        host.faults.lock().unwrap().list_unavailable = 1;
    };
    let (opened, ()) = tokio::join!(opening, fail_the_next_listing_call);
    assert!(
        opened.is_err(),
        "the open answers its listing's failure: {opened:?}"
    );
    // Past the queued re-acquire and the renewal's next tick.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let acquires = host
        .take_calls()
        .iter()
        .filter(|call| matches!(call, HostCall::AcquireFence))
        .count();
    assert_eq!(acquires, 1, "only the open acquired the fence");
}

// A listing longer than the lease (4 s, lease 3 s): the renews run beside
// the open's listing, so the writer the open returns is live and never
// stopped.
#[tokio::test]
async fn renews_keep_the_lease_through_a_slow_open_listing() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    host.faults.lock().unwrap().list_delay = Some(Duration::from_secs(4));
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("the realm opens");
    assert!(
        realm.lease_is_live(),
        "the renews during the open's listing kept the lease"
    );
    assert!(
        realm.realm_poisoned().is_none(),
        "{:?}",
        realm.realm_poisoned()
    );
    let acquires = host
        .take_calls()
        .iter()
        .filter(|call| matches!(call, HostCall::AcquireFence))
        .count();
    assert_eq!(
        acquires, 1,
        "the writer never stopped, so it never re-acquired"
    );
}
