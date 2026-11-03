//! `EventLogStore` profile: the store-facing contract Meta's buffered
//! flusher relies on.
//!
//! SCOPE PIN: the flusher itself (`EventLogHandle`, `run_flush_loop`,
//! `flush_batch`, `EVENT_LOG_RETRY_BUFFER_CAP = 4096`) is crate-private in
//! `meerkat-meta`; its retry-on-failure and oldest-dropped-at-cap behavior
//! is pinned by Meta's own unit tests
//! (`unified_runtime/event_log.rs::tests`). What a store backend can be held
//! to — and what this chapter pins — are the redelivery shapes that flusher
//! produces at the `append_batch` boundary:
//!
//! 1. identical-batch redelivery (a flush whose success reply was lost);
//! 2. compound redelivery: the failed batch plus events that arrived after
//!    the failure, in one batch (`flush_batch` restores the failed events and
//!    appends the new arrivals);
//! 3. suffix redelivery with a gap: after the retry buffer exceeds its cap,
//!    the oldest events are dropped and only a suffix is redelivered — the
//!    store must persist what it is given without erroring on the gap.
//!
//! All three reduce to the documented requirement: `append_batch` MUST be
//! idempotent on event `id`.

use std::collections::BTreeSet;

use meerkat_store_conformance::ConformanceFailure;

use crate::factory::EventLogStoreFactory;
use crate::fixtures;
use crate::steps::Steps;
use meerkat_meta::unified_runtime::EventQuery;

const CHAPTER: &str = "event_log";

pub async fn event_log(factory: &dyn EventLogStoreFactory) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    // --- identical-batch redelivery ------------------------------------------
    let step = "append_batch_idempotent_redelivery";
    let first_batch = vec![
        fixtures::persisted_event("evt-1", 1),
        fixtures::persisted_event("evt-2", 2),
        fixtures::persisted_event("evt-3", 3),
    ];
    steps.wrap(step, store.append_batch(first_batch.clone()).await)?;
    steps.wrap(step, store.append_batch(first_batch).await)?;
    let rows = steps.wrap(step, store.query(EventQuery::default()).await)?;
    assert_exactly_once(&steps, step, &rows, &["evt-1", "evt-2", "evt-3"])?;

    // --- compound post-failure redelivery -------------------------------------
    // The exact shape `flush_batch` produces after a failed flush: the failed
    // events plus everything that arrived since, in one batch.
    let step = "compound_retry_batch_exactly_once";
    let compound = vec![
        fixtures::persisted_event("evt-1", 1),
        fixtures::persisted_event("evt-2", 2),
        fixtures::persisted_event("evt-3", 3),
        fixtures::persisted_event("evt-4", 4),
        fixtures::persisted_event("evt-5", 5),
    ];
    steps.wrap(step, store.append_batch(compound).await)?;
    let rows = steps.wrap(step, store.query(EventQuery::default()).await)?;
    assert_exactly_once(
        &steps,
        step,
        &rows,
        &["evt-1", "evt-2", "evt-3", "evt-4", "evt-5"],
    )?;

    // --- post-cap suffix redelivery (gap tolerated) ----------------------------
    // After EVENT_LOG_RETRY_BUFFER_CAP enforcement the oldest events are gone;
    // the redelivered batch starts mid-stream. The store must persist the
    // suffix without erroring on the sequence gap.
    let step = "post_cap_suffix_redelivery";
    let suffix = vec![
        fixtures::persisted_event("evt-4", 4),
        fixtures::persisted_event("evt-5", 5),
        fixtures::persisted_event("evt-8", 8),
        fixtures::persisted_event("evt-9", 9),
    ];
    steps.wrap(step, store.append_batch(suffix).await)?;
    let rows = steps.wrap(step, store.query(EventQuery::default()).await)?;
    assert_exactly_once(
        &steps,
        step,
        &rows,
        &[
            "evt-1", "evt-2", "evt-3", "evt-4", "evt-5", "evt-8", "evt-9",
        ],
    )?;

    // --- after_seq cursor semantics ---------------------------------------------
    let step = "after_seq_exclusive_cursor";
    let paged = steps.wrap(
        step,
        store
            .query(EventQuery {
                after_seq: Some(3),
                ..EventQuery::default()
            })
            .await,
    )?;
    steps.ensure(
        step,
        paged.iter().all(|event| event.seq > 3),
        "after_seq is an EXCLUSIVE lower bound: only events with seq > after_seq may return",
    )?;
    assert_exactly_once(&steps, step, &paged, &["evt-4", "evt-5", "evt-8", "evt-9"])?;
    steps.ensure(
        step,
        paged.windows(2).all(|pair| pair[0].seq < pair[1].seq),
        "query results must be ordered by seq so the last-seen seq is a valid next cursor",
    )?;
    let limited = steps.wrap(
        step,
        store
            .query(EventQuery {
                after_seq: Some(3),
                limit: Some(2),
                ..EventQuery::default()
            })
            .await,
    )?;
    steps.ensure(
        step,
        limited.len() == 2,
        format!(
            "limit must bound the page size (expected 2, got {})",
            limited.len()
        ),
    )?;
    Ok(())
}

fn assert_exactly_once(
    steps: &Steps,
    step: &'static str,
    rows: &[meerkat_meta::unified_runtime::PersistedEvent],
    expected_ids: &[&str],
) -> Result<(), ConformanceFailure> {
    let mut seen = BTreeSet::new();
    for row in rows {
        steps.ensure(
            step,
            seen.insert(row.id.clone()),
            format!(
                "event id {} appears more than once — redelivery must be idempotent",
                row.id
            ),
        )?;
    }
    for id in expected_ids {
        steps.ensure(
            step,
            seen.contains(*id),
            format!("event id {id} must be present exactly once"),
        )?;
    }
    steps.ensure(
        step,
        seen.len() == expected_ids.len(),
        format!(
            "expected exactly {} distinct events, found {}",
            expected_ids.len(),
            seen.len()
        ),
    )?;
    Ok(())
}
