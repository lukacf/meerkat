#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::{SessionId, ToolCredentialContextRef};
use meerkat_jobs::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, DetachedJobService,
    DetachedJobStore, ExecutionIntentId, InteractionLineageId, JobProgress, JobSpec,
    JobSubmissionKey, JobTerminalResult, MemoryDetachedJobStore, PredicateComparison,
    PredicateDeliveryIdempotencyKey, PredicateDeliveryIdentity, PredicateObservation,
    PredicatePollingPolicy, PredicateSource, PredicateWatch, PredicateWatchId, RestartClass,
    RunnerHandleRef, RunnerIdentity, RunnerSpecificationRef, ScheduleIdRef, SqliteDetachedJobStore,
    ToolIdentity, WorkerId,
};

fn spec(key: &str, restart_class: RestartClass) -> JobSpec {
    JobSpec::new(
        "realm-a",
        SessionId::new(),
        ExecutionIntentId::new(),
        InteractionLineageId::new(),
        ToolIdentity::new("security_scan", "v1").expect("valid tool identity"),
        RunnerIdentity::new("homecore.security_scan", "v1").expect("valid runner identity"),
        restart_class,
        CanonicalArgumentsHash::new("sha256:scan-a").expect("valid arguments hash"),
        JobSubmissionKey::new(key).expect("valid submission key"),
    )
    .with_runner_specification_ref(
        RunnerSpecificationRef::new("sha256:shell-runner-spec").expect("runner specification ref"),
    )
}

#[tokio::test]
async fn sqlite_reopen_preserves_committed_writer_authority_without_advancing_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    let store = Arc::new(SqliteDetachedJobStore::open(&path).expect("open"));
    let service = DetachedJobService::new(store.clone());
    let receipt = service
        .submit(spec("sqlite-reopen", RestartClass::Adoptable))
        .await
        .expect("submit");
    let claim = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker"),
                100,
                10_000,
                RunnerHandleRef::new("external:scan-42").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    service
        .report_progress(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            JobProgress::new(1, "before restart").expect("progress"),
            9_000,
        )
        .await
        .expect("progress");
    let before = service
        .get(&receipt.job_id)
        .await
        .expect("get")
        .expect("job");
    drop(service);
    drop(store);

    let reopened_store = Arc::new(SqliteDetachedJobStore::open(&path).expect("reopen"));
    let reopened = DetachedJobService::new(reopened_store.clone());
    let recovered = reopened
        .get(&receipt.job_id)
        .await
        .expect("get")
        .expect("job");

    assert_eq!(recovered.revision, before.revision);
    assert_eq!(recovered.attempt_count, claim.attempt_count);
    assert_eq!(
        recovered.current_attempt_id.as_ref(),
        Some(&claim.attempt_id)
    );
    assert_eq!(recovered.current_fence, claim.fence);
    assert_eq!(recovered.lease_expires_at_ms, Some(10_000));
    assert_eq!(
        recovered
            .runner_handle
            .as_ref()
            .map(RunnerHandleRef::as_str),
        Some("external:scan-42")
    );
    assert_eq!(
        reopened_store
            .get(&receipt.job_id)
            .await
            .expect("stored job")
            .expect("job")
            .spec
            .runner_specification_ref
            .as_ref()
            .map(RunnerSpecificationRef::as_str),
        Some("sha256:shell-runner-spec")
    );

    reopened
        .report_progress(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            JobProgress::new(2, "after restart").expect("progress"),
            9_100,
        )
        .await
        .expect("reopen alone must not fence the latest committed writer");
}

#[tokio::test]
async fn sqlite_predicate_delivery_rolls_back_job_and_receipt_when_receipt_insert_fails() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    let store = Arc::new(SqliteDetachedJobStore::open(&path).expect("open"));
    let service = DetachedJobService::new(store);
    let job_id = service
        .submit(spec(
            "sqlite-predicate-atomicity",
            RestartClass::CheckpointResumable,
        ))
        .await
        .expect("submit")
        .job_id;
    let claim = service
        .claim_attempt(
            &job_id,
            AttemptClaim::new(
                WorkerId::new("predicate-worker").expect("worker"),
                100,
                10_000,
                RunnerHandleRef::new("predicate-runner").expect("runner handle"),
            ),
        )
        .await
        .expect("claim");
    let write = AttemptWriteAuthority::from(&claim);
    let watch = PredicateWatch::scheduled(
        PredicateWatchId::new("sqlite-atomic-watch").expect("watch id"),
        ScheduleIdRef::new("sqlite-atomic-schedule").expect("schedule id"),
        PredicateSource::StableHttp {
            url: "https://example.invalid/sqlite-atomic".into(),
            conditional_requests: true,
        },
        PredicateComparison::Changed,
        PredicatePollingPolicy::new(60, 1, 0, 1, 300).expect("policy"),
    )
    .expect("watch");
    let identity = PredicateDeliveryIdentity::new(
        PredicateDeliveryIdempotencyKey::new("schedule:sqlite-atomic-schedule:occurrence:first")
            .expect("delivery key"),
        "occurrence:first",
        "meerkat.predicate.evaluate.v1",
    )
    .expect("delivery identity");
    let before = service
        .get(&job_id)
        .await
        .expect("get before")
        .expect("job before");

    let conn = rusqlite::Connection::open(&path).expect("raw open");
    conn.execute_batch(
        "CREATE TRIGGER fail_predicate_receipt_insert
         BEFORE INSERT ON detached_job_predicate_deliveries
         BEGIN
             SELECT RAISE(ABORT, 'forced predicate receipt failure');
         END;",
    )
    .expect("failure trigger");
    drop(conn);

    service
        .evaluate_predicate_idempotent(
            &job_id,
            write.clone(),
            identity.clone(),
            &watch,
            PredicateObservation::available("v1", "Version v1").expect("observation"),
            200,
        )
        .await
        .expect_err("receipt insertion failure must abort the whole transaction");
    let after_failure = service
        .get(&job_id)
        .await
        .expect("get after failure")
        .expect("job after failure");
    assert_eq!(after_failure.revision, before.revision);
    assert_eq!(after_failure.checkpoint_ref, before.checkpoint_ref);
    assert_eq!(after_failure.progress, before.progress);
    assert_eq!(after_failure.outbox, before.outbox);
    assert!(
        !service
            .predicate_delivery_applied(&job_id, &identity)
            .await
            .expect("no receipt after rollback")
    );

    let conn = rusqlite::Connection::open(&path).expect("raw reopen");
    conn.execute_batch("DROP TRIGGER fail_predicate_receipt_insert;")
        .expect("drop failure trigger");
    drop(conn);
    service
        .evaluate_predicate_idempotent(
            &job_id,
            write,
            identity,
            &watch,
            PredicateObservation::available("v1", "Version v1").expect("observation"),
            300,
        )
        .await
        .expect("retry after rollback");
}

#[tokio::test]
async fn sqlite_origin_listing_survives_reopen_and_reports_persistent_storage() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    let session_id = SessionId::new();
    let store = Arc::new(SqliteDetachedJobStore::open(&path).expect("open"));
    let service = DetachedJobService::new(store.clone());
    let mut submitted = spec("sqlite-list", RestartClass::NonResumable);
    submitted.origin_session_id = session_id.clone();
    let expected = service.submit(submitted).await.expect("submit").job_id;
    drop(service);
    drop(store);

    let reopened = SqliteDetachedJobStore::open(&path).expect("reopen");
    assert!(reopened.is_persistent());
    let listed = reopened
        .list_for_origin("realm-a", &session_id, 10)
        .await
        .expect("list");

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].job_id, expected);
}

#[tokio::test]
async fn sqlite_deduplicates_submission_and_cas_allows_one_revision_writer() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(
        SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("open store"),
    );
    let service = DetachedJobService::new(store.clone());
    let submitted = spec("sqlite-dedup-cas", RestartClass::NonResumable)
        .with_credential_context_refs(vec![ToolCredentialContextRef::OwningProfile {
            required_scopes: ["network".to_string()].into_iter().collect(),
        }]);
    let first = service.submit(submitted.clone()).await.expect("submit");
    let second = service
        .submit(submitted.clone())
        .await
        .expect("deduplicate");
    assert_eq!(second.job_id, first.job_id);
    assert!(second.deduplicated);
    assert_eq!(
        store
            .get(&first.job_id)
            .await
            .expect("read")
            .expect("job")
            .spec
            .credential_context_refs,
        submitted.credential_context_refs
    );

    let left = store.get(&first.job_id).await.expect("read").expect("job");
    let right = left.clone();
    let revision = left.revision;
    let (left_result, right_result) = tokio::join!(
        DetachedJobStore::compare_and_swap(&*store, revision, left),
        DetachedJobStore::compare_and_swap(&*store, revision, right),
    );
    assert_ne!(left_result.is_ok(), right_result.is_ok());
    let revision_type: String = rusqlite::Connection::open(store.path())
        .expect("raw open")
        .query_row(
            "SELECT typeof(revision) FROM detached_jobs WHERE job_id = ?1",
            [first.job_id.as_str()],
            |row| row.get(0),
        )
        .expect("revision storage type");
    assert_eq!(
        revision_type, "blob",
        "the full u64 revision domain must not be truncated to SQLite's signed integer range"
    );
}

#[tokio::test]
async fn sqlite_terminal_state_and_pending_outbox_commit_and_reopen_together() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    let store = Arc::new(SqliteDetachedJobStore::open(&path).expect("open"));
    let service = DetachedJobService::new(store.clone());
    let receipt = service
        .submit(spec("sqlite-outbox", RestartClass::Replayable))
        .await
        .expect("submit");
    let claim = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker"),
                100,
                1_000,
                RunnerHandleRef::new("process:42").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    service
        .complete_attempt(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            900,
            None,
        )
        .await
        .expect("complete");
    drop(service);
    drop(store);

    let reopened = SqliteDetachedJobStore::open(&path).expect("reopen");
    let stored = reopened
        .get(&receipt.job_id)
        .await
        .expect("get")
        .expect("job");
    assert_eq!(
        stored.terminal_result,
        Some(JobTerminalResult::Succeeded { result_ref: None })
    );
    assert_eq!(stored.outbox.len(), 1);
    assert!(!stored.outbox[0].applied);

    let pending = reopened
        .list_pending_outbox(10)
        .await
        .expect("pending outbox");
    assert_eq!(pending, stored.outbox);
}

#[test]
fn sqlite_open_stamps_the_jobs_schema_domain() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    SqliteDetachedJobStore::open(&path).expect("open");

    let versions = meerkat_sqlite::domain_version(
        &meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::ReadOnly)
            .expect("read-only open"),
        meerkat_jobs::JOBS_DOMAIN.name,
    )
    .expect("domain version");
    assert_eq!(
        versions,
        Some(meerkat_jobs::JOBS_DOMAIN.supported_version())
    );
}

#[test]
fn sqlite_open_refuses_a_jobs_schema_below_the_0_8_10_floor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    let conn = rusqlite::Connection::open(&path).expect("raw open");
    conn.execute_batch(
        "CREATE TABLE meerkat_schema (
             domain TEXT PRIMARY KEY,
             version INTEGER NOT NULL
         );
         INSERT INTO meerkat_schema (domain, version) VALUES ('jobs', 1);
         CREATE TABLE detached_jobs (
             job_id TEXT PRIMARY KEY,
             realm_id TEXT NOT NULL,
             submission_key TEXT NOT NULL,
             revision BLOB NOT NULL CHECK (length(revision) = 8),
             has_pending_outbox INTEGER NOT NULL CHECK (has_pending_outbox IN (0, 1)),
             job_json BLOB NOT NULL,
             UNIQUE (realm_id, submission_key)
         );
         CREATE INDEX idx_detached_jobs_pending_outbox
             ON detached_jobs (has_pending_outbox, job_id);",
    )
    .expect("legacy schema");
    drop(conn);

    let error = SqliteDetachedJobStore::open(&path)
        .expect_err("jobs schema v1 predates the 0.8.10 compatibility floor");
    assert!(matches!(
        error,
        meerkat_jobs::DetachedJobError::Sqlite(
            meerkat_sqlite::SqliteStoreError::UnsupportedSchemaPredecessor {
                ref domain,
                found: 1,
                supported: 4,
                ref allowed,
            }
        ) if domain == "jobs" && allowed == &[2, 3, 4]
    ));
    let version = meerkat_sqlite::domain_version(
        &meerkat_sqlite::open(&path, meerkat_sqlite::ConnectionProfile::ReadOnly)
            .expect("read-only open"),
        meerkat_jobs::JOBS_DOMAIN.name,
    )
    .expect("domain version");
    assert_eq!(version, Some(1), "refusal must not mutate the ledger");
}

#[test]
fn sqlite_open_migrates_the_released_v2_jobs_schema_to_the_predicate_ledger() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    let conn = rusqlite::Connection::open(&path).expect("raw open");
    conn.execute_batch(
        "CREATE TABLE meerkat_schema (
             domain TEXT PRIMARY KEY,
             version INTEGER NOT NULL
         );
         INSERT INTO meerkat_schema (domain, version) VALUES ('jobs', 2);
         CREATE TABLE detached_jobs (
             job_id TEXT PRIMARY KEY,
             realm_id TEXT NOT NULL,
             submission_key TEXT NOT NULL,
             revision BLOB NOT NULL CHECK (length(revision) = 8),
             has_pending_outbox INTEGER NOT NULL CHECK (has_pending_outbox IN (0, 1)),
             job_json BLOB NOT NULL,
             UNIQUE (realm_id, submission_key)
         );
         CREATE INDEX idx_detached_jobs_pending_outbox
             ON detached_jobs (has_pending_outbox, job_id);",
    )
    .expect("released v2 schema");
    drop(conn);

    SqliteDetachedJobStore::open(&path).expect("v2 migration");
    let conn = rusqlite::Connection::open(&path).expect("raw reopen");
    let ledger_table: String = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
            ["detached_job_predicate_deliveries"],
            |row| row.get(0),
        )
        .expect("predicate ledger table");
    assert_eq!(ledger_table, "detached_job_predicate_deliveries");
    let version = meerkat_sqlite::domain_version(&conn, meerkat_jobs::JOBS_DOMAIN.name)
        .expect("domain version");
    assert_eq!(version, Some(4));
}

#[test]
fn sqlite_open_refuses_a_future_jobs_schema() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    let conn = rusqlite::Connection::open(&path).expect("raw open");
    conn.execute_batch(
        "CREATE TABLE meerkat_schema (
             domain TEXT PRIMARY KEY,
             version INTEGER NOT NULL
         );
         INSERT INTO meerkat_schema (domain, version) VALUES ('jobs', 999);",
    )
    .expect("future ledger");
    drop(conn);

    let error = SqliteDetachedJobStore::open(&path)
        .expect_err("an older jobs store must refuse a future domain");
    assert!(matches!(
        error,
        meerkat_jobs::DetachedJobError::Sqlite(
            meerkat_sqlite::SqliteStoreError::SchemaFromTheFuture {
                ref domain,
                found: 999,
                supported: 4
            }
        ) if domain == "jobs"
    ));
}

#[tokio::test]
async fn sqlite_rejects_an_unknown_stored_job_envelope_version() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    let store = SqliteDetachedJobStore::open(&path).expect("open");
    let service = DetachedJobService::new(Arc::new(store.clone()));
    let receipt = service
        .submit(spec("future-envelope", RestartClass::NonResumable))
        .await
        .expect("submit");

    let conn = rusqlite::Connection::open(&path).expect("raw open");
    let encoded: Vec<u8> = conn
        .query_row(
            "SELECT job_json FROM detached_jobs WHERE job_id = ?1",
            [receipt.job_id.as_str()],
            |row| row.get(0),
        )
        .expect("stored envelope");
    let mut envelope: serde_json::Value = serde_json::from_slice(&encoded).expect("json");
    envelope["format_version"] = serde_json::json!(2);
    conn.execute(
        "UPDATE detached_jobs SET job_json = ?2 WHERE job_id = ?1",
        rusqlite::params![
            receipt.job_id.as_str(),
            serde_json::to_vec(&envelope).expect("encode")
        ],
    )
    .expect("corrupt version");
    drop(conn);

    let error = store
        .get(&receipt.job_id)
        .await
        .expect_err("unknown persisted envelopes must fail closed");
    assert!(
        error
            .to_string()
            .contains("format version 2 is unsupported")
    );
}

#[tokio::test]
async fn sqlite_reads_the_versioned_envelope_from_text_or_blob_json_columns() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    let store = SqliteDetachedJobStore::open(&path).expect("open");
    let service = DetachedJobService::new(Arc::new(store.clone()));
    let receipt = service
        .submit(spec("text-envelope", RestartClass::NonResumable))
        .await
        .expect("submit");
    let conn = rusqlite::Connection::open(&path).expect("raw open");
    conn.execute(
        "UPDATE detached_jobs SET job_json = CAST(job_json AS TEXT) WHERE job_id = ?1",
        [receipt.job_id.as_str()],
    )
    .expect("convert JSON storage class");
    drop(conn);

    store
        .get(&receipt.job_id)
        .await
        .expect("TEXT JSON is accepted")
        .expect("job");
}

async fn pending_outbox_ack_conformance(store: Arc<dyn DetachedJobStore>, key: &str) {
    let service = DetachedJobService::new(store.clone());
    let receipt = service
        .submit(spec(key, RestartClass::Replayable))
        .await
        .expect("submit");
    let claim = service
        .claim_attempt(
            &receipt.job_id,
            AttemptClaim::new(
                WorkerId::new("worker-a").expect("worker"),
                100,
                1_000,
                RunnerHandleRef::new("process:42").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    service
        .complete_attempt(
            &receipt.job_id,
            AttemptWriteAuthority::from(&claim),
            900,
            None,
        )
        .await
        .expect("complete");
    let pending = store.list_pending_outbox(10).await.expect("pending");
    assert_eq!(pending.len(), 1);
    service
        .mark_delivery_applied(&receipt.job_id, pending[0].delivery_sequence)
        .await
        .expect("acknowledge");
    assert!(
        store
            .list_pending_outbox(10)
            .await
            .expect("pending after ack")
            .is_empty()
    );
}

#[tokio::test]
async fn memory_and_sqlite_share_pending_outbox_ack_conformance() {
    pending_outbox_ack_conformance(
        Arc::new(MemoryDetachedJobStore::new()),
        "memory-pending-conformance",
    )
    .await;
    let temp = tempfile::tempdir().expect("tempdir");
    pending_outbox_ack_conformance(
        Arc::new(
            SqliteDetachedJobStore::open(temp.path().join("jobs.sqlite3")).expect("sqlite open"),
        ),
        "sqlite-pending-conformance",
    )
    .await;
}

/// A `census_live` column that disagrees with the document fails the read.
///
/// The column is a projection of the phase, and a projection with no staleness
/// policy is a second source of truth. The failure mode it would otherwise
/// produce is invisible by construction - a live row marked settled simply
/// stops appearing in the health window - so the guard has to fire on any read
/// that touches the row rather than waiting for the census to notice an
/// absence.
#[tokio::test]
async fn a_stale_census_live_column_fails_closed_on_read() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("jobs.sqlite3");
    let store = SqliteDetachedJobStore::open(&path).expect("open");
    let service = DetachedJobService::new(Arc::new(store.clone()));
    let receipt = service
        .submit(spec("census-drift", RestartClass::NonResumable))
        .await
        .expect("submit");
    assert!(
        DetachedJobStore::get(&store, &receipt.job_id)
            .await
            .expect("read before tampering")
            .is_some()
    );

    let conn = rusqlite::Connection::open(&path).expect("raw open");
    let changed = conn
        .execute(
            "UPDATE detached_jobs SET census_live = 0 WHERE job_id = ?1",
            [receipt.job_id.as_str()],
        )
        .expect("tamper with the projection");
    assert_eq!(changed, 1);
    drop(conn);

    let error = DetachedJobStore::get(&store, &receipt.job_id)
        .await
        .expect_err("a queued job marked settled is a projection that lies");
    assert!(
        format!("{error}").contains("disagree"),
        "the refusal must name the disagreement: {error}"
    );
}
