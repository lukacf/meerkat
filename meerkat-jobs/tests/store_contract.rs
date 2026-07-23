#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_jobs::{
    CanonicalArgumentsHash, DetachedJobService, ExecutionIntentId, InteractionLineageId, JobSpec,
    JobSubmissionKey, MemoryDetachedJobStore, OriginMemberId, RestartClass, RunnerIdentity,
    ToolIdentity,
};

fn spec(key: &str) -> JobSpec {
    spec_in_realm("realm-a", key)
}

fn spec_in_realm(realm: &str, key: &str) -> JobSpec {
    JobSpec::new(
        realm,
        SessionId::new(),
        ExecutionIntentId::new(),
        InteractionLineageId::new(),
        ToolIdentity::new("scan", "v1").expect("tool"),
        RunnerIdentity::new("runner.scan", "v1").expect("runner"),
        RestartClass::NonResumable,
        CanonicalArgumentsHash::new("sha256:args").expect("hash"),
        JobSubmissionKey::new(key).expect("key"),
    )
}

#[tokio::test]
async fn submission_deduplication_never_crosses_realm_boundaries() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());

    let first = service
        .submit(spec_in_realm("realm-a", "shared-key"))
        .await
        .expect("first realm");
    let second = service
        .submit(spec_in_realm("realm-b", "shared-key"))
        .await
        .expect("second realm");

    assert_ne!(first.job_id, second.job_id);
    assert!(!second.deduplicated);
    assert_eq!(store.len().await, 2);
}

#[tokio::test]
async fn compare_and_swap_allows_exactly_one_writer_for_a_revision() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let job_id = service
        .submit(spec("cas-race"))
        .await
        .expect("submit")
        .job_id;
    let first = store.get(&job_id).await.expect("read").expect("job");
    let second = first.clone();
    let revision = first.revision;

    let (left, right) = tokio::join!(
        meerkat_jobs::DetachedJobStore::compare_and_swap(&*store, revision, first),
        meerkat_jobs::DetachedJobStore::compare_and_swap(&*store, revision, second),
    );

    assert_ne!(
        left.is_ok(),
        right.is_ok(),
        "exactly one CAS writer must win"
    );
    let error = left
        .err()
        .or_else(|| right.err())
        .expect("one stale writer");
    assert!(matches!(
        error,
        meerkat_jobs::DetachedJobError::StaleRevision { .. }
    ));
}

#[tokio::test]
async fn compare_and_swap_cannot_rewrite_the_submitted_job_specification() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let job_id = service
        .submit(spec("immutable-spec"))
        .await
        .expect("submit")
        .job_id;
    let mut replacement = store.get(&job_id).await.expect("read").expect("job");
    let revision = replacement.revision;
    replacement.spec.realm_id = "realm-b".into();

    let error = meerkat_jobs::DetachedJobStore::compare_and_swap(&*store, revision, replacement)
        .await
        .expect_err("CAS must not rewrite identity or submission specification");

    assert!(matches!(error, meerkat_jobs::DetachedJobError::Store(_)));
}

#[tokio::test]
async fn store_rejects_a_projection_that_disagrees_with_generated_authority() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let job_id = service
        .submit(spec("authority-projection"))
        .await
        .expect("submit")
        .job_id;
    let mut replacement = store.get(&job_id).await.expect("read").expect("job");
    let revision = replacement.revision;
    replacement.machine_state.job_id = "different-job".into();

    let error = meerkat_jobs::DetachedJobStore::compare_and_swap(&*store, revision, replacement)
        .await
        .expect_err("store must reject projection/authority disagreement");

    assert!(matches!(error, meerkat_jobs::DetachedJobError::Store(_)));
}

#[tokio::test]
async fn malformed_realm_identity_is_rejected_before_storage() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store);

    let error = service
        .submit(spec_in_realm(" realm-a", "bad-realm"))
        .await
        .expect_err("realm identity must be canonical");
    assert!(matches!(
        error,
        meerkat_jobs::DetachedJobError::InvalidInput(_)
    ));
}

#[tokio::test]
async fn submission_identity_includes_origin_member_and_credential_context_references() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store);
    let first_spec = spec("full-submission-identity")
        .with_origin_member_id(OriginMemberId::new("member-a").expect("member id"));
    let mut conflicting_spec = first_spec.clone();
    conflicting_spec.origin_member_id = Some(OriginMemberId::new("member-b").expect("member id"));

    service.submit(first_spec).await.expect("first submit");
    let error = service
        .submit(conflicting_spec)
        .await
        .expect_err("same submission key cannot alias a different origin identity");
    assert!(matches!(
        error,
        meerkat_jobs::DetachedJobError::SubmissionConflict
    ));
}

#[tokio::test]
async fn duplicate_submission_key_returns_the_original_job_without_mutation() {
    let store = Arc::new(MemoryDetachedJobStore::new());
    let service = DetachedJobService::new(store.clone());
    let submission = spec("same-intent");
    let first = service.submit(submission.clone()).await.expect("submit");
    let revision = store
        .get(&first.job_id)
        .await
        .expect("read")
        .expect("job")
        .revision;

    let duplicate = service.submit(submission).await.expect("deduplicate");

    assert_eq!(duplicate.job_id, first.job_id);
    assert!(duplicate.deduplicated);
    assert_eq!(
        store
            .get(&first.job_id)
            .await
            .expect("read")
            .expect("job")
            .revision,
        revision,
        "deduplication is a read of already committed authority"
    );
    assert_eq!(store.len().await, 1);
}
