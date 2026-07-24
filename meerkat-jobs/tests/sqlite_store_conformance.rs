#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::{SessionId, ToolCredentialContextRef};
use meerkat_jobs::{
    AttemptClaim, AttemptWriteAuthority, CanonicalArgumentsHash, DetachedJobService,
    DetachedJobStore, ExecutionIntentId, InteractionLineageId, JobProgress, JobSpec,
    JobSubmissionKey, JobTerminalResult, MemoryDetachedJobStore, RestartClass, RunnerHandleRef,
    RunnerIdentity, RunnerSpecificationRef, SqliteDetachedJobStore, ToolIdentity, WorkerId,
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
                supported: 1
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
