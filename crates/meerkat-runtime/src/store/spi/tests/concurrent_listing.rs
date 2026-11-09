//! A listing of the realm's scopes, at the open and at a re-acquire, lists
//! the four scope kinds concurrently, each kind's pages in order (a page's
//! cursor comes from the one before): it waits for its slowest kind, not
//! for the sum of the kinds. A store whose listing is slow (a downstream
//! host's took 15 to 78 s a call) then keeps its writer stopped for the
//! slowest kind's time at a re-acquire, not for every kind's in a row.

use super::*;

/// How long each listing call of the rows' store takes.
const PAGE: Duration = Duration::from_secs(1);

// Each listing call takes 1 s, and a fresh store holds no scope, so the open
// lists each of the four kinds in one call. Concurrent, the listing takes
// one call's time; one kind after another, it took four.
#[tokio::test(start_paused = true)]
async fn the_open_lists_the_scope_kinds_concurrently() {
    let host = Arc::new(TestHost::new());
    host.faults.lock().unwrap().list_delay_each = Some(PAGE);
    let started = tokio::time::Instant::now();
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("the realm opens");
    let elapsed = started.elapsed();
    let listings = host
        .take_calls()
        .into_iter()
        .filter(|call| matches!(call, HostCall::ListHeads { .. }))
        .count();
    assert_eq!(listings, 4, "one call per kind");
    assert!(
        elapsed >= PAGE && elapsed < PAGE * 2,
        "the open waited for its slowest kind, not the sum: {elapsed:?}"
    );
    assert!(realm.realm_poisoned().is_none());
}

// A downstream host's shape: the session kind on two pages, every other kind
// on one. The writer writes two runtimes (two session scopes); then every
// listing call takes 1 s and answers one head a page. A renew that never
// answers stops the writer at 2 s, and the tick at 2 s re-acquires: its
// listing waits for the session kind's two pages, one after the other (2 s),
// and no longer for the five calls in a row (5 s).
#[tokio::test(start_paused = true)]
async fn a_reacquire_waits_for_its_slowest_kind_whose_pages_stay_in_order() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    let host = Arc::new(TestHost::with_timing(timing));
    let (realm, store) = open(&host).await;
    for name in ["first", "second"] {
        store
            .persist_input_state(
                &LogicalRuntimeId::new(name),
                &persistable(StoredInputState::new_accepted(InputId::new())),
            )
            .await
            .expect("the runtime writes");
    }
    assert_eq!(
        host.scopes_of(ScopeKind::Session).len(),
        2,
        "two session scopes, so two session pages of one head"
    );
    let first = realm.fence();
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_hang = 1;
        faults.list_delay_each = Some(PAGE);
        faults.list_page_cap = Some(1);
    }
    host.take_calls();
    // 1 s: the renew never answers. 2 s: the realm gives up on it and stops,
    // and the tick at 2 s starts the re-acquire.
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert!(
        !realm.lease_is_live(),
        "the timed-out renew stopped the writer"
    );
    let stopped = tokio::time::Instant::now();
    while !realm.lease_is_live() && stopped.elapsed() < Duration::from_secs(10) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let resumed_after = stopped.elapsed();
    assert!(
        resumed_after >= Duration::from_millis(1800) && resumed_after < Duration::from_secs(3),
        "the re-acquire waited for its slowest kind, the session kind's two pages in order: \
         {resumed_after:?}"
    );
    assert!(realm.fence() > first, "a fresh token");
    let listings: Vec<ScopeKind> = host
        .take_calls()
        .into_iter()
        .filter_map(|call| match call {
            HostCall::ListHeads { kind } => Some(kind),
            _ => None,
        })
        .collect();
    assert_eq!(
        (
            listings.len(),
            listings
                .iter()
                .filter(|kind| **kind == ScopeKind::Session)
                .count()
        ),
        (5, 2),
        "two session pages and one page of every other kind: {listings:?}"
    );
}
