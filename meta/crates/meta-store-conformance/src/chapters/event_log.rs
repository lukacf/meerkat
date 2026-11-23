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
//! 2. compound redelivery: the failed events plus events that arrived after
//!    the failure, in requests of at most `batch_size` events, oldest first
//!    (`flush_batch` keeps a refused request and every later event buffered
//!    in order, so one request can carry both);
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
    // A shape `flush_batch` produces after a failed flush: the failed events
    // plus events that arrived since, in one request of at most `batch_size`.
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

    // --- the query's mob and incarnation are selected before its limit ----------
    // A host that serves a second mob keeps that mob's rows under its id and
    // the incarnation they came from; `meta/query_events` asks for one
    // selection and a client pages through it by `after_seq`. A store that
    // cuts to the limit among every mob's rows hands such a client short or
    // empty pages while its mob still has rows.
    let step = "limit_is_cut_among_the_rows_of_the_querys_mob";
    let mob = "conformance-mob";
    let earlier = meerkat_core::service::MobIncarnationId::mint();
    let now = meerkat_core::service::MobIncarnationId::mint();
    steps.wrap(
        step,
        store
            .append_batch(vec![
                fixtures::persisted_event_of_mob("evt-10", 10, mob, earlier),
                fixtures::persisted_event_of_mob("evt-11", 11, mob, now),
                fixtures::persisted_event("evt-12", 12),
                fixtures::persisted_event_of_mob("evt-13", 13, mob, earlier),
                fixtures::persisted_event_of_mob("evt-14", 14, "another-conformance-mob", now),
                fixtures::persisted_event_of_mob("evt-15", 15, mob, now),
                fixtures::persisted_event_of_mob("evt-16", 16, mob, now),
            ])
            .await,
    )?;
    let ids = |rows: &[meerkat_meta::unified_runtime::PersistedEvent]| -> Vec<String> {
        rows.iter().map(|row| row.id.clone()).collect()
    };
    let of_the_mob_now = |after_seq: Option<u64>| EventQuery {
        mob_id: Some(mob.to_string()),
        mob_incarnation: Some(now),
        after_seq,
        limit: Some(2),
        ..EventQuery::default()
    };
    let first_page = steps.wrap(step, store.query(of_the_mob_now(None)).await)?;
    steps.ensure(
        step,
        ids(&first_page) == ["evt-11", "evt-15"],
        format!(
            "a query that names a mob and its incarnation with a limit of 2 must answer the \
             first two rows of THAT selection (evt-11, evt-15), got {:?}: select by \
             `EventQuery.mob_id` and `EventQuery.mob_incarnation` before cutting to `limit`",
            ids(&first_page)
        ),
    )?;
    let second_page = steps.wrap(step, store.query(of_the_mob_now(Some(15))).await)?;
    steps.ensure(
        step,
        ids(&second_page) == ["evt-16"],
        format!(
            "the page after seq 15 of that selection must be its last row (evt-16), got {:?}",
            ids(&second_page)
        ),
    )?;
    let of_no_mob = steps.wrap(
        step,
        store
            .query(EventQuery {
                after_seq: Some(9),
                limit: Some(1),
                ..EventQuery::default()
            })
            .await,
    )?;
    steps.ensure(
        step,
        ids(&of_no_mob) == ["evt-12"],
        format!(
            "a query that names no mob selects the rows that name no mob: after seq 9 with a \
             limit of 1 it must answer evt-12, got {:?}",
            ids(&of_no_mob)
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
