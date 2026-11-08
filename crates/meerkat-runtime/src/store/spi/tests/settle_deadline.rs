//! #1550: a commit sends nothing past its token's self-fence deadline.
//!
//! A commit settles an unknown or unavailable outcome by sending its batch
//! again, with the same commit id, within its retry budget. The batch
//! carries the writer's fence token, and the writer's lease at the store
//! lasts from its last acquire or renew; the writer's self-fence window, the
//! lease minus the margin, ends before that lease can. Once the lease ends,
//! another writer may acquire, and a store that only hides the old token's
//! batches at load (outside the contract, which refuses them at commit)
//! would acknowledge a retry it never shows. So a batch is sent only while
//! the self-fence window of the token it carries is open, as a renew of that
//! token keeps it, and while the realm holds its fence: the settle stops
//! there, typed, with the batch's outcome unknown, and the scopes it touched
//! are poisoned like those of any batch that does not settle.

use super::*;

fn fast_host() -> Arc<TestHost> {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3)).unwrap();
    Arc::new(TestHost::with_timing(timing))
}

/// When each commit the host saw since the last take arrived.
fn commit_attempts(host: &TestHost) -> Vec<tokio::time::Instant> {
    host.take_calls()
        .into_iter()
        .filter_map(|call| match call {
            HostCall::Commit { at, .. } => Some(at),
            _ => None,
        })
        .collect()
}

// Unrun until the candidate's validation. Every renew fails, so the window of
// the writer's token, counted from its open, closes at 2 s; every commit
// answers `Unavailable`. A write at the open retries on the doubling pauses
// (about 0, 0.1, 0.3, 0.7 and 1.5 s); its next attempt would come at or after
// 2 s, so the settle stops there with the batch's outcome unknown, and the
// runtime's scope is poisoned while the realm keeps its fence. Before, the
// settle went on for the whole 30 s retry budget, past the lease the store
// recorded (3 s).
#[tokio::test(start_paused = true)]
async fn a_commit_sends_nothing_past_its_tokens_self_fence_deadline() {
    use meerkat_core::host_store::SelfFence;
    let host = fast_host();
    let (realm, store) = open(&host).await;
    let window = realm.window();
    let deadline = tokio::time::Instant::now() - window.elapsed + window.window;
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1000;
        faults.unavailable = 1000;
    }
    host.take_calls();
    let runtime = LogicalRuntimeId::new("late");
    let written = store
        .persist_input_state(
            &runtime,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await;
    let attempts = commit_attempts(&host);
    assert!(
        attempts.len() >= 2,
        "the commit retried within its window: {attempts:?}"
    );
    let late: Vec<Duration> = attempts
        .iter()
        .filter(|at| **at >= deadline)
        .map(|at| *at - deadline)
        .collect();
    assert!(
        late.is_empty(),
        "attempts were sent past the token's self-fence deadline, by {late:?}"
    );
    assert!(
        matches!(
            written,
            Err(RuntimeStoreError::UnsettledAtFenceWindow { .. })
        ),
        "the settle stopped typed, its outcome unknown: {written:?}"
    );
    let refused = store.load_input_states(&runtime).await;
    assert!(
        matches!(&refused, Err(RuntimeStoreError::ScopeRefused { scopes })
            if scopes.len() == 1 && scopes[0].scope == "rt:late"),
        "the unsettled batch's scope is poisoned: {refused:?}"
    );
    assert!(
        realm.realm_poisoned().is_none(),
        "the realm keeps its fence: {:?}",
        realm.realm_poisoned()
    );
}

// Unrun until the candidate's validation. Renews succeed, so the writer's
// window stays open; every commit answers `Unavailable`. While a write
// settles, the realm loses its fence (here an adapter poisons it, as a renew
// that another writer fenced out does). The settle sends nothing more, and
// the write ends with the realm's refusal. Before, it went on sending the old
// token's batch for its whole retry budget.
#[tokio::test(start_paused = true)]
async fn a_settle_sends_nothing_once_the_realm_lost_its_fence() {
    let host = Arc::new(TestHost::new());
    let (realm, store) = open(&host).await;
    host.faults.lock().unwrap().unavailable = 1000;
    host.take_calls();
    let started = host.commits_started.load(Ordering::Acquire);
    let runtime = LogicalRuntimeId::new("fenced");
    let record = persistable(StoredInputState::new_accepted(InputId::new()));
    let (written, poisoned_at) =
        tokio::join!(store.persist_input_state(&runtime, &record), async {
            while host.commits_started.load(Ordering::Acquire) < started + 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            realm.poison_all("another writer took the fence over".into());
            tokio::time::Instant::now()
        });
    let attempts = commit_attempts(&host);
    let late: Vec<Duration> = attempts
        .iter()
        .filter(|at| **at > poisoned_at)
        .map(|at| *at - poisoned_at)
        .collect();
    assert!(
        late.is_empty(),
        "attempts were sent after the realm lost its fence, by {late:?}"
    );
    assert!(
        matches!(written, Err(RuntimeStoreError::RealmRefused { .. })),
        "{written:?}"
    );
}

// Unrun until the candidate's validation. Renews succeed every second, and
// each moves the deadline of the writer's token: it never passes while the
// store answers renews. The first six commits answer `Unavailable`, for
// longer than one window (2 s): the settle goes on past the deadline it
// started under, and the write lands on its seventh attempt (at 6.3 s or
// later). A deadline fixed when the settle started would have stopped it at
// 2 s.
#[tokio::test(start_paused = true)]
async fn a_commit_keeps_retrying_while_renews_move_its_deadline() {
    use meerkat_core::host_store::SelfFence;
    let host = fast_host();
    let (realm, store) = open(&host).await;
    let window = realm.window();
    let first_deadline = tokio::time::Instant::now() - window.elapsed + window.window;
    host.faults.lock().unwrap().unavailable = 6;
    host.take_calls();
    store
        .persist_input_state(
            &LogicalRuntimeId::new("renewed"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("the batch lands once the store answers");
    let attempts = commit_attempts(&host);
    assert_eq!(
        attempts.len(),
        7,
        "six refusals, then the batch lands: {attempts:?}"
    );
    assert!(
        attempts.last().is_some_and(|at| *at > first_deadline),
        "the settle went on past the deadline it started under: {attempts:?}"
    );
    assert!(realm.realm_poisoned().is_none());
    assert!(realm.lease_is_live());
}

// Unrun until the candidate's validation. The stop is typed at the realm, for
// an adapter that commits through it directly: it names the token the batch
// carried, the attempts it sent and the last answer, the scope it touched is
// poisoned with that account, and the realm keeps its fence.
#[tokio::test(start_paused = true)]
async fn a_settle_stopped_at_its_fence_window_names_its_token_attempts_and_last_answer() {
    let host = fast_host();
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("realm opens");
    let token = realm.fence();
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1000;
        faults.unavailable = 1000;
    }
    host.take_calls();
    let id = ScopeId::new(TEST_FAMILY, "late");
    let outcome = commit_one(
        &realm,
        &id,
        ScopeDelta::Changes(vec![("pin".into(), Some(Arc::new(b"value".to_vec())))]),
    )
    .await;
    let sent = commit_attempts(&host).len();
    assert!(sent >= 2, "the batch was retried within its window");
    assert!(
        matches!(
            &outcome,
            Err(SpiError::UnsettledAtFenceWindow {
                token: stopped,
                attempts,
                last: Some(HostStoreError::Unavailable(_)),
            }) if *stopped == token && *attempts as usize == sent
        ),
        "{outcome:?} after {sent} attempts"
    );
    let poisoned = realm.poisoned_scopes();
    assert_eq!(poisoned.len(), 1, "{poisoned:?}");
    assert_eq!(poisoned[0].id, id);
    assert_eq!(
        poisoned[0].cause,
        outcome
            .err()
            .map(|error| error.to_string())
            .unwrap_or_default()
    );
    assert!(realm.realm_poisoned().is_none());
}

// Unrun until the candidate's validation. A batch stopped at its fence window
// is what the reopen loads: here the store applied the batch, then answered
// its first attempt `Indeterminate`, failed every head read and answered every
// retry `Unavailable`, so the writer could not tell it landed. The settle
// stops at the deadline with the outcome unknown and the runtime's scope is
// poisoned; a new process, once the old lease ran out, loads the batch the
// store holds. The writer never rolled anything back and never wrote it again.
#[tokio::test(start_paused = true)]
async fn the_reopen_loads_a_batch_stopped_at_its_fence_window_that_landed() {
    let host = fast_host();
    let opened = open(&host).await;
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 1000;
        faults.indeterminate_after_apply = 1;
        faults.load_head_unavailable = 1000;
        faults.landed_retry_unavailable = 1000;
    }
    let runtime = LogicalRuntimeId::new("landed");
    let written = opened
        .1
        .persist_input_state(
            &runtime,
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await;
    assert!(
        matches!(
            written,
            Err(RuntimeStoreError::UnsettledAtFenceWindow { .. })
        ),
        "{written:?}"
    );
    assert_eq!(host.commit_count(), 1, "the batch landed once");
    assert!(matches!(
        opened.1.load_input_states(&runtime).await,
        Err(RuntimeStoreError::ScopeRefused { .. })
    ));
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_unavailable = 0;
        faults.load_head_unavailable = 0;
        faults.landed_retry_unavailable = 0;
    }
    let reopened = reopen(&host, opened).await;
    assert_eq!(
        reopened
            .1
            .load_input_states(&runtime)
            .await
            .expect("the reopened realm serves the runtime")
            .len(),
        1,
        "the reopen loads the batch that landed"
    );
    assert_eq!(host.commit_count(), 1, "nothing was written again");
}

// Every renew fails, so the window of the open's token closes at 2 s. A
// large value commits its chunk batch first, and the store holds that batch
// until half a second past the deadline: the main batch's first send would
// come past the deadline, so it is never sent. The settle stops typed, with
// no attempt and no answer, and the realm keeps its fence. Without the
// deadline check before a batch's first send, the main batch went out late.
#[tokio::test(start_paused = true)]
async fn a_batch_whose_first_send_would_come_past_its_tokens_deadline_is_never_sent() {
    use meerkat_core::host_store::SelfFence;
    let host = fast_host();
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("realm opens");
    let token = realm.fence();
    let window = realm.window();
    let deadline = tokio::time::Instant::now() - window.elapsed + window.window;
    host.faults.lock().unwrap().renew_unavailable = 1000;
    let gate = Arc::new(tokio::sync::Notify::new());
    *host.commit_gate.lock().unwrap() = Some(Arc::clone(&gate));
    host.take_calls();
    let id = ScopeId::new(TEST_FAMILY, "big");
    let big = Arc::new(vec![7_u8; INLINE_VALUE_LIMIT + 1]);
    let (outcome, ()) = tokio::join!(
        commit_one(
            &realm,
            &id,
            ScopeDelta::Changes(vec![("row/big".into(), Some(big))])
        ),
        async {
            tokio::time::sleep_until(deadline + Duration::from_millis(500)).await;
            *host.commit_gate.lock().unwrap() = None;
            gate.notify_one();
        }
    );
    let attempts = commit_attempts(&host);
    assert_eq!(
        attempts.len(),
        1,
        "only the chunk batch was sent, before the deadline: {attempts:?}"
    );
    assert!(
        attempts.iter().all(|at| *at < deadline),
        "the chunk batch went out before the deadline: {attempts:?}"
    );
    assert!(
        matches!(
            &outcome,
            Err(SpiError::UnsettledAtFenceWindow {
                token: stopped,
                attempts: 0,
                last: None,
            }) if *stopped == token
        ),
        "the main batch stops typed, with no attempt and no answer: {outcome:?}"
    );
    assert!(
        realm.realm_poisoned().is_none(),
        "the realm keeps its fence: {:?}",
        realm.realm_poisoned()
    );
}

// The renew in flight never answers, so the window of the open's token stays
// open until that renew times out at its deadline (2 s), and every commit
// answers `Unavailable`. A settle that starts 100 ms before the deadline
// sends nothing at or past it and stops typed. With the deadline counted
// from the settle's start instead of the token's last acquire or renew, it
// went on past the deadline.
#[tokio::test(start_paused = true)]
async fn a_settle_that_starts_late_in_its_window_stops_at_its_tokens_deadline() {
    use meerkat_core::host_store::SelfFence;
    let host = fast_host();
    let config = SpiRealmConfig {
        renew_timeout: Duration::from_secs(1),
        ..SpiRealmConfig::default()
    };
    let realm = SpiRealm::open(Arc::clone(&host) as _, config)
        .await
        .expect("realm opens");
    let store = SpiRuntimeStore::open(Arc::clone(&realm))
        .await
        .expect("store opens");
    let window = realm.window();
    let deadline = tokio::time::Instant::now() - window.elapsed + window.window;
    {
        let mut faults = host.faults.lock().unwrap();
        faults.renew_hang = 1000;
        faults.unavailable = 1000;
    }
    tokio::time::sleep_until(deadline - Duration::from_millis(100)).await;
    assert!(realm.lease_is_live(), "the window is still open");
    host.take_calls();
    let written = store
        .persist_input_state(
            &LogicalRuntimeId::new("late-start"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await;
    let attempts = commit_attempts(&host);
    assert!(!attempts.is_empty(), "the batch was sent");
    let late: Vec<Duration> = attempts
        .iter()
        .filter(|at| **at >= deadline)
        .map(|at| *at - deadline)
        .collect();
    assert!(
        late.is_empty(),
        "attempts were sent past the token's deadline, by {late:?}"
    );
    assert!(
        matches!(
            written,
            Err(RuntimeStoreError::UnsettledAtFenceWindow { .. })
        ),
        "the settle stops typed at the token's deadline: {written:?}"
    );
}
