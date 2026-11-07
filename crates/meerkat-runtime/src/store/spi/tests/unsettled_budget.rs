//! A batch whose outcome stays unknown past its retry budget is judged by
//! the reopen alone.
//!
//! An adapter may answer a commit `Indeterminate` (the batch may or may not
//! have landed) and then, as its outage goes on, `Unavailable` to every retry
//! and every head read. No answer settles the batch: the settle runs its whole
//! retry budget while the writer's window stays open, then fails the write,
//! typed (`RuntimeStoreError::UnsettledPastRetryBudget`), and poisons the
//! scopes the batch touched, and the realm keeps its fence. The writer judges
//! nothing itself, neither from the last answer nor once the store answers
//! again: the poisoned scope refuses until a reopen loads what the store
//! holds.
//!
//! The rows run on real time, as the realm's clock is the runtime's clock on
//! this branch and the system clock before it, with a short retry budget.

use crate::store::InputStateRow;

use super::*;

/// The rows' retry budget: a few retries, well inside the writer's 20 s
/// self-fence window, so the budget alone ends the settle.
const BUDGET: Duration = Duration::from_secs(2);

/// The runtime each row writes.
fn runtime() -> LogicalRuntimeId {
    LogicalRuntimeId::new("unsettled")
}

/// Open a realm over `host` with the rows' retry budget and write one input
/// state for [`runtime`] while the store answers the batch's first attempt
/// `Indeterminate`, having applied it when `applied`, and then `Unavailable`
/// to every retry and head read. Checks that the settle ran its retry budget,
/// failed the write and poisoned the runtime's scope only, and that the realm
/// still refuses the scope once the store answers again. Returns the realm
/// and store, and the input the write carried.
async fn unsettled_past_its_budget(
    host: &Arc<TestHost>,
    applied: bool,
) -> ((Arc<SpiRealm>, SpiRuntimeStore), InputId) {
    let realm = SpiRealm::open(
        Arc::clone(host) as _,
        SpiRealmConfig {
            commit_retry_budget: BUDGET,
            ..SpiRealmConfig::default()
        },
    )
    .await
    .expect("realm opens");
    let store = SpiRuntimeStore::open(Arc::clone(&realm))
        .await
        .expect("store opens");
    let opened = (realm, store);
    {
        let mut faults = host.faults.lock().unwrap();
        if applied {
            faults.indeterminate_after_apply = 1;
            faults.landed_retry_unavailable = 1000;
        } else {
            faults.indeterminate_without_apply = 1;
            faults.unavailable = 1000;
        }
        faults.load_head_unavailable = 1000;
    }
    host.take_calls();
    let input = InputId::new();
    let started = std::time::Instant::now();
    let written = opened
        .1
        .persist_input_state(
            &runtime(),
            &persistable(StoredInputState::new_accepted(input.clone())),
        )
        .await;
    let elapsed = started.elapsed();
    let calls = host.take_calls();
    let commits = calls
        .iter()
        .filter(|call| matches!(call, HostCall::Commit { .. }))
        .count();
    let head_reads = calls
        .iter()
        .filter(|call| matches!(call, HostCall::LoadHead { .. }))
        .count();
    assert!(
        commits >= 2 && head_reads >= 1,
        "the answers changed mid-settle: {commits} attempts, {head_reads} head reads"
    );
    assert!(
        matches!(&written, Err(RuntimeStoreError::UnsettledPastRetryBudget { cause })
            if cause.ends_with("last answer: unavailable: injected)")),
        "the write fails typed, its outcome unknown, with the last answer: {written:?}"
    );
    // The settle ends once its next pause (3.2 s at most) would pass the
    // budget; the writer's window, open throughout, never stopped it.
    assert!(
        elapsed < BUDGET && elapsed + Duration::from_millis(3200) >= BUDGET,
        "the retry budget ended the settle, after {elapsed:?}"
    );
    assert_eq!(
        host.commit_count(),
        u64::from(applied),
        "the store holds the batch only when it applied it"
    );
    {
        let mut faults = host.faults.lock().unwrap();
        faults.landed_retry_unavailable = 0;
        faults.unavailable = 0;
        faults.load_head_unavailable = 0;
    }
    // The store answers again: the realm still refuses the scope, and serves
    // the others.
    let refused = opened.1.load_input_states(&runtime()).await;
    assert!(
        matches!(&refused, Err(RuntimeStoreError::ScopeRefused { scopes })
            if scopes.len() == 1 && scopes[0].scope == "rt:unsettled"),
        "the unsettled batch's scope stays poisoned: {refused:?}"
    );
    assert!(
        opened.0.realm_poisoned().is_none(),
        "{:?}",
        opened.0.realm_poisoned()
    );
    assert!(opened.0.lease_is_live());
    opened
        .1
        .persist_input_state(
            &LogicalRuntimeId::new("other"),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("another runtime writes");
    (opened, input)
}

// The store applied the batch, so a reopen loads it, written once, and the
// runtime writes on.
#[tokio::test]
async fn a_batch_that_landed_unsettled_past_its_budget_is_judged_by_the_reopen() {
    let host = Arc::new(TestHost::new());
    let (opened, input) = unsettled_past_its_budget(&host, true).await;
    let commits = host.commit_count();
    let reopened = reopen(&host, opened).await;
    let rows = reopened
        .1
        .load_input_states(&runtime())
        .await
        .expect("the reopened realm serves the runtime");
    assert!(
        matches!(rows.as_slice(), [InputStateRow::Decoded(row)] if row.state.input_id == input),
        "the reopen loads the batch the store holds: {rows:?}"
    );
    assert_eq!(host.commit_count(), commits, "nothing was written again");
    reopened
        .1
        .persist_input_state(
            &runtime(),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("the runtime writes on");
}

// The store applied nothing, though it first answered `Indeterminate`, so a
// reopen loads no input for the runtime, writes nothing in its place, and the
// runtime writes again.
#[tokio::test]
async fn a_batch_that_never_landed_unsettled_past_its_budget_is_judged_by_the_reopen() {
    let host = Arc::new(TestHost::new());
    let (opened, _input) = unsettled_past_its_budget(&host, false).await;
    let commits = host.commit_count();
    let reopened = reopen(&host, opened).await;
    let rows = reopened
        .1
        .load_input_states(&runtime())
        .await
        .expect("the reopened realm serves the runtime");
    assert!(rows.is_empty(), "the reopen loads no input: {rows:?}");
    assert_eq!(host.commit_count(), commits, "nothing was written again");
    reopened
        .1
        .persist_input_state(
            &runtime(),
            &persistable(StoredInputState::new_accepted(InputId::new())),
        )
        .await
        .expect("the runtime writes again");
}
