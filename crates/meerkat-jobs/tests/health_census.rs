//! What the operational census may claim, and about which rows.
//!
//! The defect these pin: the census read a window of `ORDER BY job_id LIMIT n`
//! and reported plain counts off it. `job_id` is a time-ordered primary key,
//! so the window fills with the OLDEST rows, which in a long-lived store are
//! terminal rows that contribute nothing - and the newest live wedged job is
//! behind them. That degrades with AGE, not load, and it degrades silently:
//! `jobs: ok` published off rows nobody looked at.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_jobs::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, DetachedJobService,
    DetachedJobStore, ExecutionIntentId, InteractionLineageId, JobFailureCode, JobHealthCoverage,
    JobHealthReading, JobId, JobResultRef, JobSpec, JobSubmissionKey, RestartClass,
    RunnerHandleRef, RunnerIdentity, SqliteDetachedJobStore, ToolIdentity, WorkerId,
};

fn spec(realm_id: &str, key: &str) -> JobSpec {
    JobSpec::new(
        realm_id,
        SessionId::new(),
        ExecutionIntentId::new(),
        InteractionLineageId::new(),
        ToolIdentity::new("security_scan", "v1").expect("tool identity"),
        RunnerIdentity::new("homecore.security_scan", "v1").expect("runner identity"),
        RestartClass::NonResumable,
        CanonicalArgumentsHash::new("sha256:scan-a").expect("arguments hash"),
        JobSubmissionKey::new(key).expect("submission key"),
    )
}

/// Drive one job all the way to `Succeeded` with its terminal delivery
/// applied: a row that is durably present and operationally finished.
async fn drained_terminal_job(service: &DetachedJobService, realm_id: &str, key: &str) -> JobId {
    let job_id = terminal_job_with_pending_delivery(service, realm_id, key).await;
    let snapshot = service.get(&job_id).await.expect("get").expect("present");
    let entry = snapshot.outbox.first().expect("terminal outbox entry");
    service
        .mark_delivery_applied(&job_id, entry.delivery_sequence)
        .await
        .expect("mark applied");
    job_id
}

/// Terminal, but its terminal delivery was never applied: the row is finished
/// and the delivery is still owed.
async fn terminal_job_with_pending_delivery(
    service: &DetachedJobService,
    realm_id: &str,
    key: &str,
) -> JobId {
    let receipt = service
        .submit(spec(realm_id, key))
        .await
        .expect("submit terminal job");
    let claim = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker"),
                10,
                10_000,
                RunnerHandleRef::new("external:scan").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    service
        .complete_attempt(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            20,
            Some(JobResultRef::new("artifact:result").expect("result ref")),
        )
        .await
        .expect("complete");
    receipt.job_id
}

/// A live job holding an expired lease: the wedge an operator pages on.
async fn wedged_running_job(
    service: &DetachedJobService,
    realm_id: &str,
    key: &str,
    lease_expires_at_ms: u64,
) -> JobId {
    let receipt = service
        .submit(spec(realm_id, key))
        .await
        .expect("submit live job");
    service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-b").expect("worker"),
                10,
                lease_expires_at_ms,
                RunnerHandleRef::new("external:scan").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    receipt.job_id
}

/// A census that stopped at its window has not established `ok`.
///
/// The boundary is the assertion: a scan that returns exactly `limit` rows
/// stopped AT the window and cannot know what is behind it, so `limit` rows is
/// already truncation, not the last complete reading. The same store read with
/// one more row of headroom is genuinely healthy - so the difference between
/// `Unreadable` and `Ok` here is entirely about what was looked at, which is
/// the distinction the old `is_degraded()` could not express at all.
#[tokio::test]
async fn a_window_that_filled_reports_unreadable_rather_than_ok() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("open store"),
    );
    let service = DetachedJobService::new(store);
    for index in 0..3 {
        service
            .submit(spec("realm-a", &format!("queued-{index}")))
            .await
            .expect("submit healthy queued job");
    }

    let saturated = service
        .health_snapshot_for_realm("realm-a", 1_000, 3)
        .await
        .expect("saturated census");
    assert_eq!(
        saturated.coverage,
        JobHealthCoverage::Truncated {
            scanned: 3,
            limit: 3
        },
        "a scan that came back exactly full stopped at the window, not at the end of the rows"
    );
    assert_eq!(
        saturated.reading(),
        JobHealthReading::Unreadable,
        "counts off a truncated window are not evidence of health"
    );

    let complete = service
        .health_snapshot_for_realm("realm-a", 1_000, 4)
        .await
        .expect("complete census");
    assert_eq!(complete.coverage, JobHealthCoverage::Complete);
    assert_eq!(complete.queued, 3);
    assert_eq!(
        complete.reading(),
        JobHealthReading::Ok,
        "the same store read to the end is genuinely healthy"
    );
}

/// The known negative for the rung: a store made entirely of finished work is
/// `Ok`, not `Degraded`.
///
/// Retention is not a fault. A monitor that alarmed on the existence of
/// terminal rows would page on ordinary healthy history, which is the failure
/// mode opposite to the one this lane fixes and just as useless.
#[tokio::test]
async fn a_store_of_finished_drained_work_is_healthy() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("open store"),
    );
    let service = DetachedJobService::new(store);
    for index in 0..5 {
        drained_terminal_job(&service, "realm-a", &format!("terminal-{index}")).await;
    }

    let health = service
        .health_snapshot_for_realm("realm-a", 1_000, 100)
        .await
        .expect("census");
    assert_eq!(health.coverage, JobHealthCoverage::Complete);
    assert_eq!(health.queued, 0);
    assert_eq!(health.running, 0);
    assert_eq!(health.stale_leases, 0);
    assert_eq!(health.needs_attention, 0);
    assert_eq!(health.pending_outbox_jobs, 0);
    assert_eq!(health.reading(), JobHealthReading::Ok);
}

/// The outbox backlog is exact, uncapped, phase-blind, and realm-scoped.
///
/// Each of those four words is a separate way the old scan got this wrong:
/// it counted only within the window (capped), it counted only rows the
/// window reached (inexact), and it applied the realm filter after the cap.
/// Phase-blind is the one that must NOT change: a terminal job holding an
/// unapplied terminal delivery is the canonical delivery wedge.
#[tokio::test]
async fn pending_outbox_is_counted_outside_the_scan_window() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store: Arc<dyn DetachedJobStore> = Arc::new(
        SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("open store"),
    );
    let service = DetachedJobService::new(Arc::clone(&store));
    for index in 0..4 {
        drained_terminal_job(&service, "realm-a", &format!("terminal-{index}")).await;
    }
    terminal_job_with_pending_delivery(&service, "realm-a", "owed-delivery").await;
    terminal_job_with_pending_delivery(&service, "realm-b", "other-realm-owed").await;
    // Two live rows so the phase window genuinely truncates at limit = 1,
    // proving the outbox count is taken outside it rather than alongside it.
    wedged_running_job(&service, "realm-a", "wedged-a", 100).await;
    wedged_running_job(&service, "realm-a", "wedged-b", 100).await;

    assert_eq!(
        store
            .count_pending_outbox_jobs(Some("realm-a"))
            .await
            .expect("count realm-a"),
        1,
        "the realm filter belongs in the query, not after a window"
    );
    assert_eq!(
        store
            .count_pending_outbox_jobs(Some("realm-b"))
            .await
            .expect("count realm-b"),
        1
    );
    assert_eq!(
        store
            .count_pending_outbox_jobs(None)
            .await
            .expect("count all realms"),
        2
    );

    // A window of one row cannot see both live rows, and the job that owes a
    // delivery is settled so it is not even a census candidate. The outbox
    // count is taken outside the window and stays exact either way.
    let health = service
        .health_snapshot_for_realm("realm-a", 1_000, 1)
        .await
        .expect("census");
    assert_eq!(
        health.coverage,
        JobHealthCoverage::Truncated {
            scanned: 1,
            limit: 1
        }
    );
    assert_eq!(
        health.pending_outbox_jobs, 1,
        "an owed delivery must not depend on the row landing inside the scan window"
    );
    assert_eq!(
        health.reading(),
        JobHealthReading::Unreadable,
        "the phase half of this census still did not complete"
    );
}

/// An owed delivery on its own degrades a fully-read census.
#[tokio::test]
async fn an_owed_delivery_degrades_a_complete_census() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("open store"),
    );
    let service = DetachedJobService::new(store);
    terminal_job_with_pending_delivery(&service, "realm-a", "owed-delivery").await;

    let health = service
        .health_snapshot_for_realm("realm-a", 1_000, 100)
        .await
        .expect("census");
    assert_eq!(health.coverage, JobHealthCoverage::Complete);
    assert_eq!(health.pending_outbox_jobs, 1);
    assert_eq!(health.reading(), JobHealthReading::Degraded);
}

/// Settled history does not consume the census window.
///
/// This is the defect in its original shape, made order-independent. Job ids
/// are time-ordered, so the terminal jobs created first sort FIRST under
/// `ORDER BY job_id`: a window of two rows over the old whole-table scan
/// returned two settled rows and the wedged job behind them was never seen,
/// while the census still reported counts. Here the same store with the same
/// window reads to the end of the LIVE population, so coverage is `Complete`
/// and the expired lease is found.
///
/// The discriminator is deliberately not "did we count the wedge" alone: a
/// whole-table window would answer `Truncated`/`Unreadable` here, which is
/// honest but useless. Only a live-scoped window can be both complete and
/// correct.
#[tokio::test]
async fn settled_rows_do_not_crowd_a_wedged_job_out_of_the_window() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("open store"),
    );
    let service = DetachedJobService::new(store);
    for index in 0..6 {
        drained_terminal_job(&service, "realm-a", &format!("terminal-{index}")).await;
    }
    wedged_running_job(&service, "realm-a", "wedged", 100).await;

    let health = service
        .health_snapshot_for_realm("realm-a", 1_000, 2)
        .await
        .expect("census");
    assert_eq!(
        health.coverage,
        JobHealthCoverage::Complete,
        "a window of 2 is not truncated by 6 rows that finished long ago"
    );
    assert_eq!(health.running, 1);
    assert_eq!(
        health.stale_leases, 1,
        "the wedge must be inside the window"
    );
    assert_eq!(health.reading(), JobHealthReading::Degraded);
}

/// The window still bounds LIVE work, and saturating it is still unreadable.
///
/// The cap did not go away; it stopped being a function of retention. Three
/// live jobs and a window of three is a genuine operational bound - too much
/// outstanding work to census - and it answers `Unreadable`, not counts.
#[tokio::test]
async fn a_window_filled_by_live_work_is_still_unreadable() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("open store"),
    );
    let service = DetachedJobService::new(store);
    for index in 0..4 {
        drained_terminal_job(&service, "realm-a", &format!("terminal-{index}")).await;
    }
    for index in 0..3 {
        wedged_running_job(&service, "realm-a", &format!("wedged-{index}"), 100).await;
    }

    let health = service
        .health_snapshot_for_realm("realm-a", 1_000, 3)
        .await
        .expect("census");
    assert_eq!(
        health.coverage,
        JobHealthCoverage::Truncated {
            scanned: 3,
            limit: 3
        }
    );
    assert_eq!(health.reading(), JobHealthReading::Unreadable);
}

/// Another realm's live work does not consume this realm's window.
///
/// The realm filter used to run AFTER the cap, so a busy sibling realm could
/// silently push this realm's jobs out of a window that then reported counts
/// for this realm.
#[tokio::test]
async fn another_realms_live_work_does_not_consume_this_realms_window() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("open store"),
    );
    let service = DetachedJobService::new(store);
    for index in 0..5 {
        wedged_running_job(&service, "realm-b", &format!("other-{index}"), 100).await;
    }
    wedged_running_job(&service, "realm-a", "mine", 100).await;

    let health = service
        .health_snapshot_for_realm("realm-a", 1_000, 2)
        .await
        .expect("census");
    assert_eq!(health.coverage, JobHealthCoverage::Complete);
    assert_eq!(health.stale_leases, 1);
    assert_eq!(health.reading(), JobHealthReading::Degraded);
}

/// A job parked for a human stays in the census.
///
/// `NeedsAttention` is TERMINAL to the generated machine and LIVE to this
/// census, and that gap is the trap in this whole change: a window scoped by
/// machine terminality would drop exactly the rows that exist to be noticed,
/// and `needs_attention` is one of the three terms that degrade the reading.
/// The store here is mostly finished history, so the row only survives the
/// window if the classification is the census's own.
#[tokio::test]
async fn a_job_parked_for_a_human_is_still_census_live() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("open store"),
    );
    let service = DetachedJobService::new(store);
    for index in 0..5 {
        drained_terminal_job(&service, "realm-a", &format!("terminal-{index}")).await;
    }
    let parked = service
        .submit(spec("realm-a", "parked"))
        .await
        .expect("submit");
    service
        .mark_needs_attention(
            &parked.job_id,
            10,
            JobFailureCode::new("credential_removed").expect("failure code"),
        )
        .await
        .expect("mark needs attention");

    let health = service
        .health_snapshot_for_realm("realm-a", 1_000, 2)
        .await
        .expect("census");
    assert_eq!(
        health.needs_attention, 1,
        "a job parked for a human must not be filtered out as terminal"
    );
    assert_eq!(health.coverage, JobHealthCoverage::Complete);
    assert_eq!(health.reading(), JobHealthReading::Degraded);
}

/// A wedged live job is `Degraded`, and says which term found it.
#[tokio::test]
async fn an_expired_lease_degrades_a_complete_census() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("open store"),
    );
    let service = DetachedJobService::new(store);
    wedged_running_job(&service, "realm-a", "wedged", 100).await;

    let health = service
        .health_snapshot_for_realm("realm-a", 1_000, 100)
        .await
        .expect("census");
    assert_eq!(health.running, 1);
    assert_eq!(health.stale_leases, 1);
    assert_eq!(health.pending_outbox_jobs, 0);
    assert_eq!(health.coverage, JobHealthCoverage::Complete);
    assert_eq!(health.reading(), JobHealthReading::Degraded);
}
