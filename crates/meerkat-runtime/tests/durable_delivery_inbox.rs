#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat_runtime::{
    InMemoryRuntimeStore, LogicalRuntimeId, RuntimeDeliveryAuthorityCasOutcome,
    RuntimeDeliveryAuthorityRecord, RuntimeDeliveryError, RuntimeDeliveryId, RuntimeDeliveryInbox,
    RuntimeDeliveryKind, RuntimeDeliveryStoreRecord, RuntimeDeliverySubmission, RuntimeStore,
};

fn submission(id: &str, payload: &[u8]) -> RuntimeDeliverySubmission {
    RuntimeDeliverySubmission::new(
        RuntimeDeliveryId::new(id).expect("valid delivery id"),
        RuntimeDeliveryKind::JobTerminal,
        "job_1",
        1,
        "interaction_1",
        payload.to_vec(),
    )
    .expect("valid delivery")
}

#[tokio::test]
async fn durable_inbox_reuses_the_original_sequence_and_rejects_conflicting_replay() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let inbox = RuntimeDeliveryInbox::new(store);
    let runtime_id = LogicalRuntimeId::new("rt:test:delivery");

    let first = inbox
        .submit(&runtime_id, submission("job:job_1:terminal:1", b"one"))
        .await
        .expect("first insert");
    assert_eq!(first.sequence, 1);
    assert!(!first.deduplicated);

    let duplicate = inbox
        .submit(&runtime_id, submission("job:job_1:terminal:1", b"one"))
        .await
        .expect("idempotent replay");
    assert_eq!(duplicate.sequence, first.sequence);
    assert!(duplicate.deduplicated);

    let error = inbox
        .submit(
            &runtime_id,
            submission("job:job_1:terminal:1", b"different"),
        )
        .await
        .expect_err("same identity cannot name different content");
    assert!(matches!(
        error,
        RuntimeDeliveryError::IdempotencyConflict(_)
    ));

    let pending = inbox
        .list_pending(&runtime_id, 10)
        .await
        .expect("pending feed");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].sequence, first.sequence);
    assert_eq!(pending[0].submission.payload(), b"one");
}

#[tokio::test]
async fn generated_cursor_authority_applies_each_delivery_once_and_in_order() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let inbox = RuntimeDeliveryInbox::new(store);
    let runtime_id = LogicalRuntimeId::new("rt:test:cursor");

    let first = inbox
        .submit(&runtime_id, submission("job:job_1:terminal:1", b"one"))
        .await
        .expect("first insert");
    let second = inbox
        .submit(&runtime_id, submission("job:job_2:terminal:1", b"two"))
        .await
        .expect("second insert");

    let error = inbox
        .mark_applied(&runtime_id, &second.delivery_id, second.sequence)
        .await
        .expect_err("cursor cannot skip a committed delivery");
    assert!(matches!(error, RuntimeDeliveryError::OutOfOrder { .. }));

    let applied = inbox
        .mark_applied(&runtime_id, &first.delivery_id, first.sequence)
        .await
        .expect("apply first");
    assert_eq!(applied, 1);
    let duplicate = inbox
        .mark_applied(&runtime_id, &first.delivery_id, first.sequence)
        .await
        .expect("duplicate application is idempotent");
    assert_eq!(duplicate, applied);

    let pending = inbox
        .list_pending(&runtime_id, 10)
        .await
        .expect("pending feed");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].sequence, second.sequence);

    let applied = inbox
        .mark_applied(&runtime_id, &second.delivery_id, second.sequence)
        .await
        .expect("apply second");
    assert_eq!(applied, 2);
    assert!(
        inbox
            .list_pending(&runtime_id, 10)
            .await
            .expect("empty feed")
            .is_empty()
    );
}

#[tokio::test]
async fn concurrent_replay_commits_one_row_and_returns_one_stable_sequence() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let inbox = RuntimeDeliveryInbox::new(store);
    let runtime_id = LogicalRuntimeId::new("rt:test:concurrent-replay");

    let left = inbox.submit(&runtime_id, submission("job:job_1:terminal:1", b"one"));
    let right = inbox.submit(&runtime_id, submission("job:job_1:terminal:1", b"one"));
    let (left, right) = tokio::join!(left, right);
    let left = left.expect("left submission");
    let right = right.expect("right submission");

    assert_eq!(left.sequence, right.sequence);
    assert_ne!(left.deduplicated, right.deduplicated);
    assert_eq!(
        inbox
            .list_pending(&runtime_id, 10)
            .await
            .expect("one durable row")
            .len(),
        1
    );
}

async fn seed_raw_delivery(
    store: &InMemoryRuntimeStore,
    runtime_id: &LogicalRuntimeId,
    authority: serde_json::Value,
    submission: serde_json::Value,
) {
    let outcome = store
        .compare_and_swap_runtime_delivery_authority(
            runtime_id,
            None,
            RuntimeDeliveryAuthorityRecord::from_parts(
                1,
                serde_json::to_vec(&authority).expect("authority json"),
            ),
            Some(RuntimeDeliveryStoreRecord::from_parts(
                "job:job_1:terminal:1",
                1,
                serde_json::to_vec(&submission).expect("submission json"),
            )),
        )
        .await
        .expect("seed raw delivery");
    assert!(matches!(
        outcome,
        RuntimeDeliveryAuthorityCasOutcome::Applied(_)
    ));
}

fn one_delivery_authority(source_sequence: u64) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "state": {
            "delivery_ids": ["job:job_1:terminal:1"],
            "delivery_sequences": {"job:job_1:terminal:1": 1},
            "delivery_source_sequences": {"job:job_1:terminal:1": source_sequence},
            "committed_sequences": [1],
            "next_sequence": 1,
            "applied_cursor": 0
        }
    })
}

fn raw_submission(source_sequence: u64, payload: Vec<u8>) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "submission": {
            "delivery_id": "job:job_1:terminal:1",
            "kind": "job_terminal",
            "source_id": "job_1",
            "source_sequence": source_sequence,
            "interaction_lineage_id": "interaction_1",
            "payload": payload
        }
    })
}

#[tokio::test]
async fn recovered_row_must_match_generated_source_sequence() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let runtime_id = LogicalRuntimeId::new("rt:test:corrupt-source-sequence");
    seed_raw_delivery(
        store.as_ref(),
        &runtime_id,
        one_delivery_authority(1),
        raw_submission(2, b"one".to_vec()),
    )
    .await;

    let error = RuntimeDeliveryInbox::new(store)
        .list_pending(&runtime_id, 10)
        .await
        .expect_err("row source sequence must match generated authority");
    assert!(matches!(error, RuntimeDeliveryError::Corrupt(_)));
}

#[tokio::test]
async fn recovered_submission_revalidates_constructor_invariants() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let runtime_id = LogicalRuntimeId::new("rt:test:corrupt-submission");
    seed_raw_delivery(
        store.as_ref(),
        &runtime_id,
        one_delivery_authority(1),
        raw_submission(1, Vec::new()),
    )
    .await;

    let error = RuntimeDeliveryInbox::new(store)
        .list_pending(&runtime_id, 10)
        .await
        .expect_err("persisted empty payload must fail closed");
    assert!(matches!(error, RuntimeDeliveryError::Corrupt(_)));
}

#[tokio::test]
async fn corrupt_high_water_is_rejected_without_expanding_the_numeric_range() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let runtime_id = LogicalRuntimeId::new("rt:test:corrupt-high-water");
    let outcome = store
        .compare_and_swap_runtime_delivery_authority(
            &runtime_id,
            None,
            RuntimeDeliveryAuthorityRecord::from_parts(
                1,
                serde_json::to_vec(&serde_json::json!({
                    "version": 1,
                    "state": {
                        "delivery_ids": [],
                        "delivery_sequences": {},
                        "delivery_source_sequences": {},
                        "committed_sequences": [],
                        "next_sequence": u64::MAX,
                        "applied_cursor": 0
                    }
                }))
                .expect("authority json"),
            ),
            None,
        )
        .await
        .expect("seed corrupt authority");
    assert!(matches!(
        outcome,
        RuntimeDeliveryAuthorityCasOutcome::Applied(_)
    ));

    let error = RuntimeDeliveryInbox::new(store)
        .applied_cursor(&runtime_id)
        .await
        .expect_err("impossible high-water mark must fail closed");
    assert!(matches!(error, RuntimeDeliveryError::Corrupt(_)));
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_reopen_rehydrates_delivery_identity_sequence_and_cursor_without_advancing_them() {
    use meerkat_runtime::SqliteRuntimeStore;

    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("runtime.sqlite3");
    let runtime_id = LogicalRuntimeId::new("rt:test:sqlite-reopen");

    let first = {
        let store = Arc::new(SqliteRuntimeStore::new(&path).expect("open sqlite"));
        let inbox = RuntimeDeliveryInbox::new(store);
        let first = inbox
            .submit(&runtime_id, submission("job:job_1:terminal:1", b"one"))
            .await
            .expect("first insert");
        inbox
            .mark_applied(&runtime_id, &first.delivery_id, first.sequence)
            .await
            .expect("apply first");
        first
    };

    let store = Arc::new(SqliteRuntimeStore::new(&path).expect("reopen sqlite"));
    let inbox = RuntimeDeliveryInbox::new(store);
    let duplicate = inbox
        .submit(&runtime_id, submission("job:job_1:terminal:1", b"one"))
        .await
        .expect("replay after reopen");
    assert_eq!(duplicate.sequence, first.sequence);
    assert!(duplicate.deduplicated);
    assert_eq!(
        inbox
            .applied_cursor(&runtime_id)
            .await
            .expect("recovered cursor"),
        first.sequence
    );

    let next = inbox
        .submit(&runtime_id, submission("job:job_2:terminal:1", b"two"))
        .await
        .expect("next insert");
    assert_eq!(next.sequence, first.sequence + 1);
}
