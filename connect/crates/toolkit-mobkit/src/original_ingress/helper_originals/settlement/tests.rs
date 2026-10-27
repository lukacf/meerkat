//! Actual original-owner Candidate/admit/drop controls. The existing source
//! fixture supplies WorkGraph identity without the native operation fence.
//! The preparation timeout schedules call the actual PrivateOriginals entry,
//! with no runtime installed; they do not claim a complete native helper turn.
use super::*;
use super::super::tests::{admitted_source_fixture, fixture, pending_source};
use futures::FutureExt;
use meerkat_core::service::WorkGraphNamespaceGrant;
use meerkat_workgraph::SqliteWorkGraphStore;

async fn pending() -> (
    tempfile::TempDir,
    Arc<ToolkitOriginalOwner>,
    Arc<RetainedToolkitOriginal>,
    PendingToolkitOriginal,
    AdmittedHelperWork,
) {
    let (directory, owner, base) = fixture();
    let (original, pending) = pending_source(&owner, &base, "settlement", "user", &[]);
    let service = WorkGraphService::with_namespace_grant(
        Arc::new(SqliteWorkGraphStore::open(directory.path().join("work.sqlite")).unwrap()),
        WorkGraphNamespaceGrant::new("test-realm", "setup").unwrap(),
    ).unwrap();
    let (admitted, _) = admitted_source_fixture(
        &owner, &service, &original, vec![original.clone()], &base,
    ).await;
    (directory, owner, original, pending, admitted)
}

fn is_candidate(owner: &ToolkitOriginalOwner, original: &Arc<RetainedToolkitOriginal>) -> bool {
    owner.originals.lock().unwrap().iter().filter_map(OriginalRecord::bound)
        .any(|held| Arc::ptr_eq(&held.retained, original)
            && held.source_custody == Some(SourceCustody::Candidate))
}

#[tokio::test]
async fn delayed_acknowledgement_allows_the_same_strict_source_without_resubmission() {
    let (_directory, owner, original, pending, admitted) = pending().await;
    let original_count = owner.originals.lock().unwrap().len();
    let item = admitted.item_id().clone();
    let wait = owner.wait_admitted_helper(&admitted, Instant::now() + Duration::from_secs(1));
    tokio::pin!(wait);
    assert!(wait.as_mut().now_or_never().is_none());
    assert!(matches!(owner.check_admitted_helper(&admitted), Err(OriginalWriterError::Unavailable)));
    // Acceptance runs independently of the waiting tool. Neither owner mutex
    // survives the first poll, so the existing guard can settle the same source.
    assert!(owner.mutation.try_lock().is_ok());
    assert!(owner.originals.try_lock().is_ok());
    assert!(Arc::ptr_eq(&pending.admit().unwrap(), &original));
    wait.await.unwrap();
    owner.check_admitted_helper(&admitted).unwrap();
    assert_eq!(owner.originals.lock().unwrap().len(), original_count);
    assert_eq!(admitted.item_id(), &item);
}

#[tokio::test]
async fn withdrawn_retry_candidate_refuses_without_promoting_source() {
    let (_directory, owner, original, pending, admitted) = pending().await;
    let wait = owner.wait_admitted_helper(&admitted, Instant::now() + Duration::from_secs(1));
    tokio::pin!(wait);
    assert!(wait.as_mut().now_or_never().is_none());
    // Actual dedup/refusal source-guard Drop, not a copied admission state.
    drop(pending);
    assert!(matches!(wait.await, Err(OriginalWriterError::WrongOwner)));
    assert!(matches!(owner.check_admitted_helper(&admitted), Err(OriginalWriterError::WrongOwner)));
    let claims = original.association().candidate();
    assert!(owner.admitted_original_basis(&claims.ingress_namespace, &claims.original_work).unwrap().is_none());
}

#[tokio::test]
async fn canonical_admitted_replay_survives_dropped_dedup_guard() {
    let (_directory, owner, original, pending, admitted) = pending().await;
    pending.admit().unwrap();
    // The actual owner returns a nonpending guard for this canonical Admitted
    // source. Dropping a dedup retry must not clear its already accepted custody.
    let retry = owner.record_candidate(&original).unwrap();
    drop(retry);
    assert!(matches!(owner.wait_admitted_helper(
        &admitted, Instant::now() + Duration::from_secs(1),
    ).now_or_never(), Some(Ok(()))));
    owner.check_admitted_helper(&admitted).unwrap();
    let claims = original.association().candidate();
    assert!(owner.admitted_original_basis(&claims.ingress_namespace, &claims.original_work).unwrap().is_some());
    assert_eq!(owner.originals.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn unresolved_source_times_out_without_withdrawal_or_deadline_restart() {
    let (_directory, owner, original, pending, admitted) = pending().await;
    let deadline = Instant::now() + Duration::from_millis(20);
    let result = tokio::time::timeout(
        Duration::from_secs(1), owner.wait_admitted_helper(&admitted, deadline),
    ).await.unwrap();
    assert!(matches!(result, Err(OriginalWriterError::Unavailable)));
    assert!(Instant::now() >= deadline);
    assert!(is_candidate(&owner, &original));
    // A late real acknowledgement settles the same guard but cannot renew an
    // expired preparation deadline.
    pending.admit().unwrap();
    assert!(matches!(owner.wait_admitted_helper(&admitted, deadline).await,
        Err(OriginalWriterError::Unavailable)));
    owner.check_admitted_helper(&admitted).unwrap();
}

#[tokio::test]
async fn acknowledged_source_contention_retries_until_release_or_the_same_deadline() {
    let (_directory, owner, _original, pending, admitted) = pending().await;
    pending.admit().unwrap();
    let wait = owner.wait_admitted_helper(&admitted, Instant::now() + Duration::from_secs(1));
    tokio::pin!(wait);
    {
        let _contended = owner.originals.lock().unwrap();
        assert!(wait.as_mut().now_or_never().is_none());
    }
    wait.await.unwrap();
    owner.check_admitted_helper(&admitted).unwrap();

    // This test deliberately keeps the synchronous mutex unavailable through
    // the deadline. The production observer only try_locks it and yields; no
    // production guard is retained by that observer across its await.
    let deadline = Instant::now() + Duration::from_millis(20);
    let _contended = owner.originals.lock().unwrap();
    assert!(matches!(owner.wait_admitted_helper(&admitted, deadline).await,
        Err(OriginalWriterError::Unavailable)));
    assert!(Instant::now() >= deadline);
}

#[tokio::test]
async fn pending_cancellation_waits_but_committed_cancellation_refuses_immediately() {
    let (directory, owner, original, _pending, admitted) = pending().await;
    let store = toolkit_store::SqliteStore::open(directory.path().join("cancel.sqlite")).unwrap();
    let source = store.admit(AdmissionRequest {
        key: StoreKey { realm: "test-realm".into(), owner: "test-source".into(), id: "source".into() },
        admission_key: "source".into(),
        data: RecordData { schema: "test.source".into(), version: 1, body: serde_json::json!({}) },
        events: vec![], effects: vec![],
    }).await.unwrap().record().clone();
    let claims = original.association().candidate();
    let expected_association = original.association().clone();
    let (release, released) = tokio::sync::oneshot::channel();
    let cancellation = owner.record_operation_cancelled(
        &claims.ingress_namespace.realm,
        &claims.original_work,
        move |binding, _| async move {
            assert_eq!(binding.as_ref(), Some(&expected_association));
            Ok(Arc::new(source))
        },
        move |_| async move {
            released.await.unwrap();
            Ok(Arc::new(store.admit(AdmissionRequest {
                key: StoreKey { realm: "test-realm".into(), owner: "test-source".into(), id: "cancel".into() },
                admission_key: "cancel".into(),
                data: RecordData { schema: "test.cancelled".into(), version: 1, body: serde_json::json!({}) },
                events: vec![], effects: vec![],
            }).await.unwrap().record().clone()))
        },
    );
    tokio::pin!(cancellation);
    assert!(cancellation.as_mut().now_or_never().is_none());
    let wait = owner.wait_admitted_helper(&admitted, Instant::now() + Duration::from_secs(1));
    tokio::pin!(wait);
    assert!(wait.as_mut().now_or_never().is_none());
    assert!(matches!(owner.check_admitted_helper(&admitted), Err(OriginalWriterError::Unavailable)));
    release.send(()).unwrap();
    cancellation.await.unwrap();
    assert!(matches!(wait.await, Err(OriginalWriterError::OperationCancelled)));
    assert!(matches!(owner.wait_admitted_helper(
        &admitted, Instant::now() + Duration::from_secs(1),
    ).now_or_never(), Some(Err(OriginalWriterError::OperationCancelled))));
}

#[tokio::test]
async fn wrong_owner_and_invalid_contributor_refuse_without_waiting_on_candidate() {
    let (_directory, owner, _original, _pending, mut admitted) = pending().await;
    let (_other_directory, other, base) = fixture();
    let deadline = Instant::now() + Duration::from_secs(1);
    assert!(matches!(other.wait_admitted_helper(&admitted, deadline).now_or_never(),
        Some(Err(OriginalWriterError::WrongOwner))));
    let (foreign, _foreign_pending) = pending_source(&other, &base, "foreign", "other-user", &[]);
    admitted.contributors.push(foreign);
    assert!(matches!(owner.wait_admitted_helper(&admitted, deadline).now_or_never(),
        Some(Err(OriginalWriterError::WrongOwner))));
}

#[tokio::test]
async fn prepare_entry_keeps_complete_outer_timeout_and_advance_absolute_timeout() {
    use crate::private_originals::{PrivateOriginalError, PrivateOriginals};
    for complete in [true, false] {
        let (directory, owner, original, pending, admitted) = pending().await;
        let store = Arc::new(toolkit_store::SqliteStore::open(directory.path().join("private.sqlite")).unwrap());
        let scope = StoreScope { realm: "test-realm".into(), owner: "private-source".into() };
        let source = Arc::new(PrivateOriginals::new(scope.clone(), store.clone(), owner.clone(), vec![]).await.unwrap());
        let before = store.list_records(&scope, None, 256).await.unwrap();
        let started = Instant::now();
        let deadline = started + if complete { Duration::from_secs(1) } else { Duration::from_millis(20) };
        let prepare = source.prepare_setup_helper(Arc::new(admitted), "read my Gmail".into(), deadline);
        if complete {
            // This is the same timeout primitive as bounded(owner.complete).
            // Cancellation of its waiter must not withdraw the original guard.
            assert!(tokio::time::timeout(Duration::from_millis(20), prepare).await.is_err());
        } else {
            let outcome = tokio::time::timeout(Duration::from_secs(1), prepare).await.unwrap();
            assert!(matches!(outcome, Err(PrivateOriginalError::Unavailable)));
            assert!(Instant::now() >= deadline);
        }
        assert!(Instant::now() - started >= Duration::from_millis(20));
        assert!(is_candidate(&owner, &original));
        assert_eq!(store.list_records(&scope, None, 256).await.unwrap(), before);
        // No runtime was installed: this is an actual preparation-entry source
        // gate control, not native session/input/turn acceptance coverage.
        assert!(Arc::ptr_eq(&pending.admit().unwrap(), &original));
    }
}
