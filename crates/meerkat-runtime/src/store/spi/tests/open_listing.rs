//! The open's listing and the writer's fence. A realm installs what its open
//! listed (the scope versions it knows) only once that listing ends, and its
//! writer's renewal runs from the open on: a re-acquire may compare a
//! listing only with a baseline the open installed, so it never reads the
//! realm's own scopes as another writer's.

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
