#![allow(clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use meerkat_runtime::{
    InMemoryRuntimeStore, LogicalRuntimeId, RuntimeDeliveryAuthorityCasOutcome,
    RuntimeDeliveryAuthorityRecord, RuntimeDeliveryError, RuntimeDeliveryId, RuntimeDeliveryInbox,
    RuntimeDeliveryKind, RuntimeDeliveryOwnerAlreadyArmed, RuntimeDeliveryStoreRecord,
    RuntimeDeliverySubmission, RuntimeStore,
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

/// The commit generation names its runtimes: the one delivery owner takes
/// every runtime that received a new row exactly once, across clones, and an
/// exact replay records nothing. A second owner is refused until the first
/// is released.
#[tokio::test]
async fn committed_runtimes_are_taken_once_by_the_one_delivery_owner() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let inbox = RuntimeDeliveryInbox::new(store);
    let producer = inbox.clone();
    let first = LogicalRuntimeId::new("rt:test:committed-a");
    let second = LogicalRuntimeId::new("rt:test:committed-b");

    producer
        .submit(&first, submission("job:early:terminal:1", b"early"))
        .await
        .expect("commit before the owner exists");
    let owner = inbox.claim_delivery_ownership().expect("first owner");
    assert!(
        owner.take_committed_runtimes().is_empty(),
        "rows committed before the claim belong to the owner's reconcile read"
    );
    assert_eq!(
        producer.claim_delivery_ownership().err(),
        Some(RuntimeDeliveryOwnerAlreadyArmed),
        "a clone cannot arm a second owner"
    );

    producer
        .submit(&first, submission("job:a:terminal:1", b"a"))
        .await
        .expect("commit a");
    producer
        .submit(&second, submission("job:b:terminal:1", b"b"))
        .await
        .expect("commit b");
    let mut taken = owner.take_committed_runtimes();
    taken.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(taken, vec![first.clone(), second]);
    assert!(
        owner.take_committed_runtimes().is_empty(),
        "taking empties the set"
    );

    let replay = producer
        .submit(&first, submission("job:a:terminal:1", b"a"))
        .await
        .expect("replay a");
    assert!(replay.deduplicated);
    assert!(
        owner.take_committed_runtimes().is_empty(),
        "an exact replay is not a new commit"
    );

    drop(owner);
    assert!(
        producer.claim_delivery_ownership().is_ok(),
        "ownership is released on drop"
    );
}

fn submission_with_source(
    id: &str,
    source_sequence: u64,
    payload: &[u8],
) -> RuntimeDeliverySubmission {
    RuntimeDeliverySubmission::new(
        RuntimeDeliveryId::new(id).expect("valid delivery id"),
        RuntimeDeliveryKind::JobTerminal,
        "job_1",
        source_sequence,
        "interaction_1",
        payload.to_vec(),
    )
    .expect("valid delivery")
}

/// Every status verdict comes from the generated authority, and a fresh inbox
/// over the same store (a restart) reads the same verdicts.
#[tokio::test]
async fn delivery_status_reports_each_verdict_from_the_durable_authority() {
    use meerkat_runtime::RuntimeDeliveryStatus;

    let store = Arc::new(InMemoryRuntimeStore::new());
    let inbox = RuntimeDeliveryInbox::new(store.clone());
    let runtime_id = LogicalRuntimeId::new("rt:test:status");
    let first = RuntimeDeliveryId::new("job:a:1").expect("id");
    let second = RuntimeDeliveryId::new("job:b:1").expect("id");

    assert_eq!(
        inbox
            .delivery_status(&runtime_id, &first)
            .await
            .expect("status"),
        RuntimeDeliveryStatus::NotCommitted
    );
    inbox
        .submit(&runtime_id, submission("job:a:1", b"a"))
        .await
        .expect("commit a");
    inbox
        .submit(&runtime_id, submission("job:b:1", b"b"))
        .await
        .expect("commit b");
    assert_eq!(
        inbox
            .delivery_status(&runtime_id, &first)
            .await
            .expect("status"),
        RuntimeDeliveryStatus::Pending {
            delivery_sequence: 1
        }
    );

    inbox
        .acknowledge(&runtime_id, &second, 2)
        .await
        .expect("acknowledge b ahead of a");
    assert_eq!(
        inbox
            .delivery_status(&runtime_id, &second)
            .await
            .expect("status"),
        RuntimeDeliveryStatus::AcknowledgedAhead {
            delivery_sequence: 2
        }
    );

    inbox
        .mark_applied(&runtime_id, &first, 1)
        .await
        .expect("apply a");
    let restarted = RuntimeDeliveryInbox::new(store);
    assert_eq!(
        restarted
            .delivery_status(&runtime_id, &first)
            .await
            .expect("status"),
        RuntimeDeliveryStatus::Applied {
            delivery_sequence: 1
        }
    );
    assert_eq!(
        restarted
            .delivery_status(&runtime_id, &second)
            .await
            .expect("status"),
        RuntimeDeliveryStatus::Applied {
            delivery_sequence: 2
        },
        "applying a carries the cursor over the acknowledged b"
    );
}

/// The same delivery id with another source sequence is the generated
/// machine's typed refusal, and the committed row stands.
#[tokio::test]
async fn a_conflicting_source_sequence_is_a_typed_conflict_and_the_row_stands() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let inbox = RuntimeDeliveryInbox::new(store);
    let runtime_id = LogicalRuntimeId::new("rt:test:source-conflict");
    inbox
        .submit(&runtime_id, submission_with_source("job:a:1", 1, b"a"))
        .await
        .expect("commit");
    let error = inbox
        .submit(&runtime_id, submission_with_source("job:a:1", 2, b"a"))
        .await
        .expect_err("another source sequence under the same id");
    assert!(
        matches!(error, RuntimeDeliveryError::IdempotencyConflict(ref id) if id.as_str() == "job:a:1"),
        "typed conflict, got {error:?}"
    );
    let pending = inbox.list_pending(&runtime_id, 10).await.expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].submission.source_sequence(), 1);
}

fn continuation_submission(id: &str, payload: &[u8]) -> RuntimeDeliverySubmission {
    RuntimeDeliverySubmission::new(
        RuntimeDeliveryId::new(id).expect("valid delivery id"),
        RuntimeDeliveryKind::Continuation,
        "host",
        1,
        "interaction_1",
        payload.to_vec(),
    )
    .expect("valid delivery")
}

fn claim(owner: &str, key: &str, digest: &str) -> meerkat_runtime::ContinuationKeyClaim {
    meerkat_runtime::ContinuationKeyClaim {
        owner: owner.into(),
        key: key.into(),
        submission_digest: digest.into(),
        committed_at_ms: 1,
    }
}

async fn keyed_submit_binds_once(
    inbox: RuntimeDeliveryInbox,
    reopen: impl Fn() -> RuntimeDeliveryInbox,
) {
    use meerkat_runtime::{KeyedSubmitOutcome, RuntimeDeliveryStatus};

    let address = LogicalRuntimeId::new("rt:session:owner-a");
    let committed = inbox
        .submit_with_key_claim(
            &address,
            continuation_submission("continuation:first", b"result"),
            claim("session:owner-a", "task-1", "digest-a"),
        )
        .await
        .expect("first keyed submit");
    let KeyedSubmitOutcome::Committed { receipt, binding } = committed else {
        panic!("the first submit commits, got {committed:?}");
    };
    assert_eq!(receipt.sequence, 1);
    assert_eq!(binding.address, address);
    assert_eq!(binding.delivery_id, "continuation:first");

    // The same (owner, key) at another address writes nothing and returns
    // the original binding.
    let other_address = LogicalRuntimeId::new("rt:session:owner-b");
    let replay = inbox
        .submit_with_key_claim(
            &other_address,
            continuation_submission("continuation:second", b"other"),
            claim("session:owner-a", "task-1", "digest-b"),
        )
        .await
        .expect("second keyed submit");
    assert_eq!(replay, KeyedSubmitOutcome::AlreadyBound(binding.clone()));
    assert!(
        inbox
            .list_pending(&other_address, 10)
            .await
            .expect("pending")
            .is_empty(),
        "an already-bound key writes no row"
    );

    let reopened = reopen();
    assert_eq!(
        reopened
            .continuation_key_binding("session:owner-a", "task-1")
            .await
            .expect("binding read"),
        Some(binding)
    );
    assert_eq!(
        reopened
            .delivery_status(
                &address,
                &RuntimeDeliveryId::new("continuation:first").expect("id")
            )
            .await
            .expect("status"),
        RuntimeDeliveryStatus::Pending {
            delivery_sequence: 1
        }
    );
}

#[tokio::test]
async fn memory_keyed_submit_binds_a_continuation_key_once() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let reopen_store = store.clone();
    keyed_submit_binds_once(RuntimeDeliveryInbox::new(store), move || {
        RuntimeDeliveryInbox::new(reopen_store.clone())
    })
    .await;
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_keyed_submit_binds_a_continuation_key_once_and_survives_reopen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("runtime.sqlite3");
    let store = Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(&path).expect("open"));
    let reopen_path = path.clone();
    keyed_submit_binds_once(RuntimeDeliveryInbox::new(store), move || {
        RuntimeDeliveryInbox::new(Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(&reopen_path).expect("reopen"),
        ))
    })
    .await;
}

/// Two submits racing one (owner, key) at different addresses: exactly one
/// binds, the other sees that binding.
#[tokio::test]
async fn racing_keyed_submits_bind_one_continuation_key() {
    use meerkat_runtime::KeyedSubmitOutcome;

    let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
    let left = {
        let inbox = inbox.clone();
        tokio::spawn(async move {
            inbox
                .submit_with_key_claim(
                    &LogicalRuntimeId::new("rt:session:left"),
                    continuation_submission("continuation:left", b"left"),
                    claim("session:owner", "race", "digest-left"),
                )
                .await
        })
    };
    let right = {
        let inbox = inbox.clone();
        tokio::spawn(async move {
            inbox
                .submit_with_key_claim(
                    &LogicalRuntimeId::new("rt:session:right"),
                    continuation_submission("continuation:right", b"right"),
                    claim("session:owner", "race", "digest-right"),
                )
                .await
        })
    };
    let outcomes = [
        left.await.expect("join").expect("left"),
        right.await.expect("join").expect("right"),
    ];
    let committed: Vec<_> = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            KeyedSubmitOutcome::Committed { binding, .. } => Some(binding.clone()),
            KeyedSubmitOutcome::AlreadyBound(_) => None,
        })
        .collect();
    assert_eq!(committed.len(), 1, "exactly one submit binds: {outcomes:?}");
    assert!(
        outcomes
            .iter()
            .any(|outcome| outcome == &KeyedSubmitOutcome::AlreadyBound(committed[0].clone()))
    );
}

/// The continuation admission index only moves by its typed transitions:
/// reserve, repoint a reservation out of the session it names, and apply in
/// the reserved session. Everything else is refused without a write, and the
/// state survives a reopen.
async fn admission_index_moves_only_by_typed_transitions(
    store: Arc<dyn RuntimeStore>,
    reopen: impl Fn() -> Arc<dyn RuntimeStore>,
) {
    use meerkat_core::lifecycle::InputId;
    use meerkat_core::types::SessionId;
    use meerkat_runtime::{
        ContinuationAdmission, ContinuationAdmissionOutcome as Outcome,
        ContinuationAdmissionTransition as T,
    };

    let address = LogicalRuntimeId::new("rt:session:admission-index");
    let delivery = "continuation:admission-index";
    let (s1, s2, s3) = (SessionId::new(), SessionId::new(), SessionId::new());
    let (i1, i2) = (InputId::new(), InputId::new());
    let step = |transition: T| {
        let store = store.clone();
        let address = address.clone();
        async move {
            store
                .transition_continuation_admission(&address, delivery, transition)
                .await
                .expect("admission index transition")
        }
    };
    let reserved_s1 = ContinuationAdmission::Reserved {
        session_id: s1.clone(),
        input_id: i1.clone(),
    };
    let rejected = |current: &ContinuationAdmission| Outcome::Rejected {
        current: Some(current.clone()),
    };

    assert_eq!(
        step(T::Apply {
            session_id: s1.clone(),
            input_id: i1.clone(),
        })
        .await,
        Outcome::Rejected { current: None },
        "nothing applies without a reservation"
    );
    let reserve_s1 = T::Reserve {
        session_id: s1.clone(),
        input_id: i1.clone(),
    };
    assert_eq!(
        step(reserve_s1.clone()).await,
        Outcome::Transitioned(reserved_s1.clone())
    );
    assert_eq!(
        step(reserve_s1).await,
        Outcome::Transitioned(reserved_s1.clone()),
        "re-reserving the same reservation is a no-op"
    );
    assert_eq!(
        step(T::Reserve {
            session_id: s2.clone(),
            input_id: i2.clone(),
        })
        .await,
        rejected(&reserved_s1),
        "a second reservation never overwrites the first"
    );
    assert_eq!(
        step(T::Repoint {
            from: s3.clone(),
            session_id: s2.clone(),
            input_id: i2.clone(),
        })
        .await,
        rejected(&reserved_s1),
        "a repoint names the session the reservation is in"
    );
    let reserved_s2 = ContinuationAdmission::Reserved {
        session_id: s2.clone(),
        input_id: i2.clone(),
    };
    assert_eq!(
        step(T::Repoint {
            from: s1.clone(),
            session_id: s2.clone(),
            input_id: i2.clone(),
        })
        .await,
        Outcome::Transitioned(reserved_s2.clone())
    );
    assert_eq!(
        step(T::Apply {
            session_id: s1.clone(),
            input_id: i1.clone(),
        })
        .await,
        rejected(&reserved_s2),
        "the superseded session is never recorded as the admission"
    );
    let applied_s2 = ContinuationAdmission::Applied {
        session_id: s2.clone(),
        input_id: i2.clone(),
    };
    let apply_s2 = T::Apply {
        session_id: s2.clone(),
        input_id: i2.clone(),
    };
    assert_eq!(
        step(apply_s2.clone()).await,
        Outcome::Transitioned(applied_s2.clone())
    );
    assert_eq!(
        step(apply_s2).await,
        Outcome::Transitioned(applied_s2.clone()),
        "re-applying the same admission is a no-op"
    );
    for after_apply in [
        T::Repoint {
            from: s2.clone(),
            session_id: s3.clone(),
            input_id: i1.clone(),
        },
        T::Apply {
            session_id: s2.clone(),
            input_id: i1.clone(),
        },
        T::Reserve {
            session_id: s3.clone(),
            input_id: i1.clone(),
        },
    ] {
        assert_eq!(
            step(after_apply).await,
            rejected(&applied_s2),
            "an applied admission is final"
        );
    }
    assert_eq!(
        reopen()
            .load_continuation_admission(&address, delivery)
            .await
            .expect("load after reopen"),
        Some(applied_s2)
    );
}

#[tokio::test]
async fn memory_admission_index_moves_only_by_typed_transitions() {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let reopen_store = store.clone();
    admission_index_moves_only_by_typed_transitions(store, move || reopen_store.clone()).await;
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_admission_index_moves_only_by_typed_transitions_and_survives_reopen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("runtime.sqlite3");
    let store: Arc<dyn RuntimeStore> =
        Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(&path).expect("open"));
    admission_index_moves_only_by_typed_transitions(store, move || {
        Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(&path).expect("reopen"))
    })
    .await;
}

/// Refused settlement: only the row at the cursor, never one acknowledged out
/// of band, never applied afterwards; repeating it observes it; the cursor
/// passes it so the next row applies; and the verdict survives a reopen.
async fn refused_settlement_passes_the_cursor_without_applying(
    store: Arc<dyn RuntimeStore>,
    reopen: impl Fn() -> Arc<dyn RuntimeStore>,
) {
    use meerkat_runtime::{RuntimeDeliveryRefusalReason as Reason, RuntimeDeliveryStatus};

    let inbox = RuntimeDeliveryInbox::new(store);
    let runtime_id = LogicalRuntimeId::new("rt:test:refusal");
    let first = RuntimeDeliveryId::new("job:a:1").expect("id");
    let second = RuntimeDeliveryId::new("job:b:1").expect("id");
    let third = RuntimeDeliveryId::new("job:c:1").expect("id");
    for id in ["job:a:1", "job:b:1", "job:c:1"] {
        inbox
            .submit(&runtime_id, submission(id, id.as_bytes()))
            .await
            .expect("commit");
    }

    assert!(
        inbox
            .mark_refused(&runtime_id, &second, 2, Reason::NoAdmissibleWorkBinding)
            .await
            .is_err(),
        "only the row at the cursor can be refused"
    );
    assert_eq!(
        inbox
            .mark_refused(&runtime_id, &first, 1, Reason::NoAdmissibleWorkBinding)
            .await
            .expect("refuse the head"),
        1,
        "the cursor passes the refused row"
    );
    assert_eq!(
        inbox
            .mark_refused(&runtime_id, &first, 1, Reason::NoAdmissibleWorkBinding)
            .await
            .expect("repeat"),
        1,
        "a repeated refusal observes the settlement"
    );
    assert!(
        inbox.mark_applied(&runtime_id, &first, 1).await.is_err(),
        "a refused row is never applied"
    );

    inbox
        .acknowledge(&runtime_id, &third, 3)
        .await
        .expect("acknowledge c ahead of b");
    assert_eq!(
        inbox
            .mark_applied(&runtime_id, &second, 2)
            .await
            .expect("the next row applies"),
        3,
        "the cursor carries over the acknowledged row"
    );
    for (applied, sequence) in [(&second, 2), (&third, 3)] {
        assert!(
            matches!(
                inbox
                    .mark_refused(
                        &runtime_id,
                        applied,
                        sequence,
                        Reason::NoAdmissibleWorkBinding
                    )
                    .await,
                Err(RuntimeDeliveryError::Authority(_))
            ),
            "an applied row has no refusal transition"
        );
    }

    let reopened = RuntimeDeliveryInbox::new(reopen());
    assert_eq!(
        reopened
            .delivery_status(&runtime_id, &first)
            .await
            .expect("status"),
        RuntimeDeliveryStatus::Refused {
            delivery_sequence: 1,
            reason: Reason::NoAdmissibleWorkBinding
        }
    );
    assert_eq!(
        reopened
            .delivery_status(&runtime_id, &second)
            .await
            .expect("status"),
        RuntimeDeliveryStatus::Applied {
            delivery_sequence: 2
        }
    );
    assert_eq!(reopened.pending_delivery_total().await.expect("backlog"), 0);
}

#[tokio::test]
async fn memory_refused_settlement_passes_the_cursor_without_applying() {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let reopen_store = store.clone();
    refused_settlement_passes_the_cursor_without_applying(store, move || reopen_store.clone())
        .await;
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn sqlite_refused_settlement_passes_the_cursor_and_survives_reopen() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("runtime.sqlite3");
    let store: Arc<dyn RuntimeStore> =
        Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(&path).expect("open"));
    refused_settlement_passes_the_cursor_without_applying(store, move || {
        Arc::new(meerkat_runtime::store::SqliteRuntimeStore::new(&path).expect("reopen"))
    })
    .await;
}

fn refused_head_authority(version: u64) -> serde_json::Value {
    serde_json::json!({
        "version": version,
        "state": {
            "delivery_ids": ["job:job_1:terminal:1"],
            "delivery_sequences": {"job:job_1:terminal:1": 1},
            "delivery_source_sequences": {"job:job_1:terminal:1": 1},
            "committed_sequences": [1],
            "next_sequence": 1,
            "applied_cursor": 1,
            "refused_deliveries": {"job:job_1:terminal:1": "no_admissible_work_binding"}
        }
    })
}

// A copy of the released v1 reader's initial version gate. This checks actual
// writer bytes against that gate, not against a full historical decoder.
fn released_v1_envelope_version_gate(bytes: &[u8]) -> Result<(), String> {
    let envelope: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    match envelope["version"].as_u64() {
        Some(1) => Ok(()),
        version => Err(format!(
            "unsupported runtime delivery authority envelope version {version:?}"
        )),
    }
}

/// Old readers fail closed: a runtime holding a refused settlement is written
/// as authority envelope version 2, which readers that predate refused
/// settlement (version 1 only) refuse instead of reading the refused row as
/// applied through the cursor. A runtime without refusals stays version 1.
#[tokio::test]
async fn a_refused_settlement_gates_the_authority_envelope_for_old_readers() {
    use meerkat_runtime::{RuntimeDeliveryRefusalReason as Reason, RuntimeDeliveryStatus};

    let store = Arc::new(InMemoryRuntimeStore::new());
    let inbox = RuntimeDeliveryInbox::new(store.clone());
    let runtime_id = LogicalRuntimeId::new("rt:test:refusal-gate");
    let first = RuntimeDeliveryId::new("job:a:1").expect("id");
    inbox
        .submit(&runtime_id, submission("job:a:1", b"a"))
        .await
        .expect("commit");
    let envelope_version = |store: Arc<InMemoryRuntimeStore>, runtime_id: LogicalRuntimeId| async move {
        let record = store
            .load_runtime_delivery_authority(&runtime_id)
            .await
            .expect("load")
            .expect("authority");
        serde_json::from_slice::<serde_json::Value>(record.state_json()).expect("json")["version"]
            .as_u64()
            .expect("version")
    };
    assert_eq!(
        envelope_version(store.clone(), runtime_id.clone()).await,
        1,
        "without refusals every released reader can read the runtime"
    );
    inbox
        .mark_refused(&runtime_id, &first, 1, Reason::NoAdmissibleWorkBinding)
        .await
        .expect("refuse");
    assert_eq!(
        envelope_version(store.clone(), runtime_id.clone()).await,
        2,
        "a refusal is persisted only under the gated envelope version"
    );
    let stored = store
        .load_runtime_delivery_authority(&runtime_id)
        .await
        .expect("load")
        .expect("authority");
    assert!(
        released_v1_envelope_version_gate(stored.state_json()).is_err(),
        "a released reader refuses the runtime instead of reading the refusal as applied"
    );

    let gated = Arc::new(InMemoryRuntimeStore::new());
    let gated_runtime = LogicalRuntimeId::new("rt:test:refusal-gate-raw");
    seed_raw_delivery(
        &gated,
        &gated_runtime,
        refused_head_authority(2),
        raw_submission(1, b"a".to_vec()),
    )
    .await;
    assert_eq!(
        RuntimeDeliveryInbox::new(gated)
            .delivery_status(
                &gated_runtime,
                &RuntimeDeliveryId::new("job:job_1:terminal:1").expect("id")
            )
            .await
            .expect("status"),
        RuntimeDeliveryStatus::Refused {
            delivery_sequence: 1,
            reason: Reason::NoAdmissibleWorkBinding
        }
    );

    let mislabeled = Arc::new(InMemoryRuntimeStore::new());
    seed_raw_delivery(
        &mislabeled,
        &gated_runtime,
        refused_head_authority(1),
        raw_submission(1, b"a".to_vec()),
    )
    .await;
    assert!(
        matches!(
            RuntimeDeliveryInbox::new(mislabeled)
                .delivery_status(
                    &gated_runtime,
                    &RuntimeDeliveryId::new("job:job_1:terminal:1").expect("id")
                )
                .await,
            Err(RuntimeDeliveryError::Corrupt(_))
        ),
        "a version 1 envelope never carries a refusal"
    );
}

fn bound_recipients() -> Vec<meerkat_runtime::RuntimeDeliveryRecipient> {
    ["a", "b", "c"]
        .into_iter()
        .map(|id| {
            meerkat_runtime::RuntimeDeliveryRecipient::new(id, format!("session:{id}:notification"))
                .expect("exact subscription binding")
        })
        .collect()
}

#[tokio::test]
async fn enrollment_uses_the_lowest_safe_envelope_and_fences_pre_recipient_versions() {
    use meerkat_runtime::{
        RuntimeDeliveryRecipientOutcome as O, RuntimeDeliveryRefusalReason as R,
    };
    // Copy the pre-recipient v2 version gate as well. Neither gate executes a
    // historical full decoder; both receive actual current writer output.
    let pre_recipient_version_gate = |bytes: &[u8]| -> Result<(), String> {
        let envelope: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        match envelope["version"].as_u64() {
            Some(1 | 2) => Ok(()),
            version => Err(format!(
                "unsupported runtime delivery authority envelope version {version:?}"
            )),
        }
    };
    for prior_refusal in [false, true] {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let runtime_id = LogicalRuntimeId::new("rt:minimum-recipient-version");
        let inbox = RuntimeDeliveryInbox::new(store.clone());
        if prior_refusal {
            let old = inbox
                .submit(&runtime_id, submission("legacy", b"single target"))
                .await
                .expect("commit legacy row");
            inbox
                .mark_refused(
                    &runtime_id,
                    &old.delivery_id,
                    old.sequence,
                    R::AuthorityDenied,
                )
                .await
                .expect("legacy refusal");
        }
        inbox
            .submit(
                &runtime_id,
                submission("group", b"unchanged committed producer payload"),
            )
            .await
            .expect("commit group");
        let record = inbox
            .list_pending(&runtime_id, 1)
            .await
            .expect("pending group")
            .remove(0);
        let before = store
            .load_runtime_delivery_authority(&runtime_id)
            .await
            .expect("read")
            .expect("authority");
        let json: serde_json::Value = serde_json::from_slice(before.state_json()).expect("JSON");
        assert_eq!(json["version"], if prior_refusal { 2 } else { 1 });
        assert!(json["state"].get("recipients").is_none());
        pre_recipient_version_gate(before.state_json())
            .expect("ordinary v1/v2 pass the pre-recipient gate");
        assert_eq!(
            released_v1_envelope_version_gate(before.state_json()).is_ok(),
            !prior_refusal,
            "the released v1 gate accepts v1 and rejects actual v2 output"
        );
        let original = store
            .load_runtime_delivery_record(&runtime_id, "group")
            .await
            .expect("read row")
            .expect("row");
        let recipients = bound_recipients();
        let bound = inbox
            .bind_recipients(&runtime_id, &record, &recipients)
            .await
            .expect("lazy enrollment");
        assert!(bound.iter().all(|recipient| recipient.outcome.is_none()));
        let enrolled = store
            .load_runtime_delivery_authority(&runtime_id)
            .await
            .expect("read")
            .expect("authority");
        let json: serde_json::Value = serde_json::from_slice(enrolled.state_json()).expect("JSON");
        assert_eq!(
            json["version"], 3,
            "enrollment itself fences old writers before any recipient enters"
        );
        assert!(pre_recipient_version_gate(enrolled.state_json()).is_err());
        assert!(released_v1_envelope_version_gate(enrolled.state_json()).is_err());
        assert_eq!(
            store
                .load_runtime_delivery_record(&runtime_id, "group")
                .await
                .expect("read row"),
            Some(original)
        );
        for recipient in &recipients {
            inbox
                .settle_recipient(&runtime_id, &record, recipient, O::Applied)
                .await
                .expect("settle success");
        }
        inbox
            .finish_recipients(&runtime_id, &record)
            .await
            .expect("finish");
        let finished = store
            .load_runtime_delivery_authority(&runtime_id)
            .await
            .expect("read")
            .expect("authority");
        assert!(
            pre_recipient_version_gate(finished.state_json()).is_err(),
            "an all-applied group still retains enrollment and cannot downgrade"
        );
        assert!(released_v1_envelope_version_gate(finished.state_json()).is_err());
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(finished.state_json()).expect("JSON")["version"],
            3
        );
    }
}

#[tokio::test]
async fn unanimous_recipient_groups_reopen_as_the_exact_existing_parent_verdict() {
    use meerkat_runtime::{
        RuntimeDeliveryRecipientOutcome as O, RuntimeDeliveryRefusalReason as R,
        RuntimeDeliveryStatus,
    };
    for (outcome, reason) in [
        (O::Applied, None),
        (O::Refused, Some(R::AuthorityDenied)),
        (
            O::OperationAuthorizationUnavailable,
            Some(R::OperationAuthorizationUnavailable),
        ),
    ] {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let runtime_id = LogicalRuntimeId::new("rt:unanimous-parent");
        let inbox = RuntimeDeliveryInbox::new(store.clone());
        inbox
            .submit(&runtime_id, submission("group", b"producer payload"))
            .await
            .expect("commit");
        let record = inbox
            .list_pending(&runtime_id, 1)
            .await
            .expect("pending")
            .remove(0);
        let recipients = bound_recipients();
        inbox
            .bind_recipients(&runtime_id, &record, &recipients)
            .await
            .expect("bind");
        for recipient in &recipients {
            inbox
                .settle_recipient(&runtime_id, &record, recipient, outcome)
                .await
                .expect("settle");
        }
        inbox
            .finish_recipients(&runtime_id, &record)
            .await
            .expect("finish");
        drop(inbox);
        let reopened = RuntimeDeliveryInbox::new(store.clone());
        let expected = match reason {
            Some(reason) => RuntimeDeliveryStatus::Refused {
                delivery_sequence: 1,
                reason,
            },
            None => RuntimeDeliveryStatus::Applied {
                delivery_sequence: 1,
            },
        };
        assert_eq!(
            reopened
                .delivery_status(&runtime_id, record.submission.delivery_id())
                .await
                .expect("read verdict"),
            expected
        );
        assert!(
            reopened
                .mark_applied(&runtime_id, record.submission.delivery_id(), 1)
                .await
                .is_err(),
            "old whole-row application cannot bypass enrolled custody"
        );
        assert!(
            reopened
                .acknowledge(&runtime_id, record.submission.delivery_id(), 1)
                .await
                .is_err(),
            "old acknowledgement cannot hide recipient truth"
        );
        assert!(
            reopened
                .mark_refused(
                    &runtime_id,
                    record.submission.delivery_id(),
                    1,
                    reason.unwrap_or(R::AuthorityDenied)
                )
                .await
                .is_err(),
            "old whole-row refusal cannot bypass enrolled custody"
        );
        let settled = reopened
            .bind_recipients(&runtime_id, &record, &recipients)
            .await
            .expect("retained exact states");
        assert!(settled.iter().all(|state| state.outcome == Some(outcome)));
        if let Some(reason) = reason {
            let observed = store
                .load_runtime_delivery_authority(&runtime_id)
                .await
                .expect("read")
                .expect("authority");
            let mut json: serde_json::Value =
                serde_json::from_slice(observed.state_json()).expect("JSON");
            assert_eq!(
                json["state"]["refused_deliveries"]["group"],
                serde_json::to_value(reason).expect("reason")
            );
            json["state"]["refused_deliveries"]
                .as_object_mut()
                .expect("refusals")
                .remove("group");
            store
                .compare_and_swap_runtime_delivery_authority(
                    &runtime_id,
                    Some(observed.revision()),
                    RuntimeDeliveryAuthorityRecord::from_parts(
                        observed.revision() + 1,
                        serde_json::to_vec(&json).expect("JSON"),
                    ),
                    None,
                )
                .await
                .expect("seed missing row disposition");
            assert!(
                matches!(
                    RuntimeDeliveryInbox::new(store)
                        .delivery_status(&runtime_id, record.submission.delivery_id())
                        .await,
                    Err(RuntimeDeliveryError::Corrupt(_))
                ),
                "cold recovery rejects a missing unanimous row refusal"
            );
        }
    }
}

async fn partially_settle_recipients(store: Arc<dyn RuntimeStore>, runtime_id: &LogicalRuntimeId) {
    use meerkat_runtime::RuntimeDeliveryRecipientOutcome as O;
    let inbox = RuntimeDeliveryInbox::new(store.clone());
    inbox
        .submit(
            runtime_id,
            submission("group", b"committed grouped payload"),
        )
        .await
        .expect("commit legacy-shaped row");
    let record = inbox
        .list_pending(runtime_id, 1)
        .await
        .expect("committed row")
        .remove(0);
    let recipients = bound_recipients();
    let states = inbox
        .bind_recipients(runtime_id, &record, &recipients)
        .await
        .expect("enroll committed group");
    assert!(states.iter().all(|state| state.outcome.is_none()));
    let original = store
        .load_runtime_delivery_record(runtime_id, "group")
        .await
        .expect("row read")
        .expect("row");
    inbox
        .settle_recipient(runtime_id, &record, &recipients[0], O::Applied)
        .await
        .expect("first application committed");
    assert!(
        inbox.finish_recipients(runtime_id, &record).await.is_err(),
        "pending recipients cannot disappear"
    );
    let mut altered = record.clone();
    altered.submission = submission("group", b"different payload");
    assert!(matches!(
        inbox
            .bind_recipients(runtime_id, &altered, &recipients)
            .await,
        Err(RuntimeDeliveryError::IdempotencyConflict(_))
    ));
    let mut changed = recipients.clone();
    changed[1] = meerkat_runtime::RuntimeDeliveryRecipient::new("b", "other-session")
        .expect("different target");
    assert!(
        inbox
            .bind_recipients(runtime_id, &record, &changed)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .load_runtime_delivery_record(runtime_id, "group")
            .await
            .expect("row read")
            .expect("row"),
        original,
        "recipient commits do not rewrite source payload"
    );
    let authority = store
        .load_runtime_delivery_authority(runtime_id)
        .await
        .expect("authority read")
        .expect("authority");
    let json: serde_json::Value =
        serde_json::from_slice(authority.state_json()).expect("authority JSON");
    assert_eq!(
        json["version"], 3,
        "even partial recipient progress fences old whole-row readers"
    );
}

async fn finish_reopened_recipients(store: Arc<dyn RuntimeStore>, runtime_id: &LogicalRuntimeId) {
    use meerkat_runtime::{
        RuntimeDeliveryRecipientGroupOutcome as G, RuntimeDeliveryRecipientOutcome as O,
        RuntimeDeliveryStatus,
    };
    let inbox = RuntimeDeliveryInbox::new(store);
    let record = inbox
        .list_pending(runtime_id, 1)
        .await
        .expect("same grouped row pending")
        .remove(0);
    let recipients = bound_recipients();
    let states = inbox
        .bind_recipients(runtime_id, &record, &recipients)
        .await
        .expect("same bindings after reopen");
    assert_eq!(
        states.iter().map(|state| state.outcome).collect::<Vec<_>>(),
        vec![Some(O::Applied), None, None],
        "retry can skip the committed success without sink deduplication"
    );
    inbox
        .settle_recipient(runtime_id, &record, &recipients[1], O::Refused)
        .await
        .expect("local policy refusal");
    inbox
        .settle_recipient(
            runtime_id,
            &record,
            &recipients[2],
            O::OperationAuthorizationUnavailable,
        )
        .await
        .expect("missing operation authority settles locally");
    assert_eq!(
        inbox
            .finish_recipients(runtime_id, &record)
            .await
            .expect("finish mixed group"),
        G::Mixed
    );
    assert_eq!(
        inbox
            .delivery_status(runtime_id, record.submission.delivery_id())
            .await
            .expect("parent status"),
        RuntimeDeliveryStatus::Mixed {
            delivery_sequence: 1,
            recipients: recipients
                .iter()
                .cloned()
                .zip([O::Applied, O::Refused, O::OperationAuthorizationUnavailable])
                .map(
                    |(recipient, outcome)| meerkat_runtime::RuntimeDeliveryRecipientState {
                        recipient,
                        outcome: Some(outcome)
                    }
                )
                .collect(),
        }
    );
    assert!(
        inbox
            .list_pending(runtime_id, 1)
            .await
            .expect("settled feed")
            .is_empty()
    );
    let states = inbox
        .bind_recipients(runtime_id, &record, &recipients)
        .await
        .expect("settled receipt remains readable");
    assert_eq!(
        states.iter().map(|state| state.outcome).collect::<Vec<_>>(),
        vec![
            Some(O::Applied),
            Some(O::Refused),
            Some(O::OperationAuthorizationUnavailable)
        ]
    );
    assert!(
        inbox
            .settle_recipient(runtime_id, &record, &recipients[1], O::Applied)
            .await
            .is_err(),
        "permission healing cannot replay a settled refusal"
    );
}

#[tokio::test]
async fn recipient_progress_survives_new_inbox_without_relabeling_mixed_parent() {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let runtime_id = LogicalRuntimeId::new("rt:recipient-progress");
    partially_settle_recipients(store.clone(), &runtime_id).await;
    finish_reopened_recipients(store, &runtime_id).await;
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn recipient_progress_survives_physical_sqlite_close_and_reopen() {
    let dir = tempfile::tempdir().expect("database directory");
    let path = dir.path().join("recipients.sqlite");
    let runtime_id = LogicalRuntimeId::new("rt:recipient-progress");
    partially_settle_recipients(
        Arc::new(meerkat_runtime::SqliteRuntimeStore::new(&path).expect("open")),
        &runtime_id,
    )
    .await;
    finish_reopened_recipients(
        Arc::new(meerkat_runtime::SqliteRuntimeStore::new(&path).expect("reopen")),
        &runtime_id,
    )
    .await;
}

#[tokio::test]
async fn recipient_state_cannot_hide_under_a_legacy_authority_version() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let runtime_id = LogicalRuntimeId::new("rt:recipient-version-fence");
    partially_settle_recipients(store.clone(), &runtime_id).await;
    let observed = store
        .load_runtime_delivery_authority(&runtime_id)
        .await
        .expect("read")
        .expect("authority");
    let mut json: serde_json::Value = serde_json::from_slice(observed.state_json()).expect("JSON");
    json["version"] = 2.into();
    store
        .compare_and_swap_runtime_delivery_authority(
            &runtime_id,
            Some(observed.revision()),
            RuntimeDeliveryAuthorityRecord::from_parts(
                observed.revision() + 1,
                serde_json::to_vec(&json).expect("JSON"),
            ),
            None,
        )
        .await
        .expect("seed corrupt version");
    assert!(matches!(
        RuntimeDeliveryInbox::new(store)
            .list_pending(&runtime_id, 1)
            .await,
        Err(RuntimeDeliveryError::Corrupt(_))
    ));
}

#[tokio::test]
async fn concurrent_recipient_commits_merge_and_terminal_replay_changes_no_revision() {
    use meerkat_runtime::{
        RuntimeDeliveryRecipientGroupOutcome as G, RuntimeDeliveryRecipientOutcome as O,
    };
    let store = Arc::new(InMemoryRuntimeStore::new());
    let inbox = RuntimeDeliveryInbox::new(store.clone());
    let runtime_id = LogicalRuntimeId::new("rt:recipient-cas");
    inbox
        .submit(&runtime_id, submission("group", b"source"))
        .await
        .expect("commit");
    let record = inbox
        .list_pending(&runtime_id, 1)
        .await
        .expect("row")
        .remove(0);
    let recipients = bound_recipients();
    let (left, right) = tokio::join!(
        inbox.bind_recipients(&runtime_id, &record, &recipients),
        inbox.bind_recipients(&runtime_id, &record, &recipients)
    );
    assert_eq!(left.expect("left binding"), right.expect("right binding"));
    let (left, right) = tokio::join!(
        inbox.settle_recipient(&runtime_id, &record, &recipients[0], O::Applied),
        inbox.settle_recipient(&runtime_id, &record, &recipients[1], O::Refused),
    );
    left.expect("first settlement");
    right.expect("second settlement");
    inbox
        .settle_recipient(&runtime_id, &record, &recipients[2], O::Applied)
        .await
        .expect("last settlement");
    let (left, right) = tokio::join!(
        inbox.finish_recipients(&runtime_id, &record),
        inbox.finish_recipients(&runtime_id, &record)
    );
    assert_eq!(left.expect("left finish"), G::Mixed);
    assert_eq!(right.expect("right finish"), G::Mixed);
    let before = store
        .load_runtime_delivery_authority(&runtime_id)
        .await
        .expect("read")
        .expect("authority");
    inbox
        .finish_recipients(&runtime_id, &record)
        .await
        .expect("repeat finish");
    inbox
        .settle_recipient(&runtime_id, &record, &recipients[0], O::Applied)
        .await
        .expect("repeat success");
    let after = store
        .load_runtime_delivery_authority(&runtime_id)
        .await
        .expect("read")
        .expect("authority");
    assert_eq!(after.revision(), before.revision());
    assert_eq!(after.state_json(), before.state_json());
}

#[tokio::test]
async fn version_three_missing_or_orphaned_recipient_state_is_corrupt_on_reopen() {
    for mutation in 0..6 {
        let store = Arc::new(InMemoryRuntimeStore::new());
        let runtime_id = LogicalRuntimeId::new("rt:recipient-corruption");
        partially_settle_recipients(store.clone(), &runtime_id).await;
        let observed = store
            .load_runtime_delivery_authority(&runtime_id)
            .await
            .expect("read")
            .expect("authority");
        let mut json: serde_json::Value =
            serde_json::from_slice(observed.state_json()).expect("JSON");
        match mutation {
            0 => {
                json["state"]
                    .as_object_mut()
                    .expect("state")
                    .remove("recipients");
            }
            1 => {
                json["state"]["recipients"]
                    .as_object_mut()
                    .expect("recipient state")
                    .remove("outcomes");
            }
            2 => {
                json["state"]["recipients"]["bindings"]
                    .as_object_mut()
                    .expect("bindings")
                    .remove("group");
            }
            3 => {
                json["state"]["recipients"]["bindings"]["group"]
                    .as_object_mut()
                    .expect("group")
                    .remove("a");
            }
            4 => {
                json["state"]["recipients"]["outcomes"]
                    .as_object_mut()
                    .expect("outcomes")
                    .remove("group");
            }
            _ => {
                let outcomes = json["state"]["recipients"]["outcomes"]
                    .as_object_mut()
                    .expect("outcomes");
                let moved = outcomes.remove("group").expect("settled recipient");
                outcomes.insert("other-delivery".into(), moved);
            }
        }
        store
            .compare_and_swap_runtime_delivery_authority(
                &runtime_id,
                Some(observed.revision()),
                RuntimeDeliveryAuthorityRecord::from_parts(
                    observed.revision() + 1,
                    serde_json::to_vec(&json).expect("JSON"),
                ),
                None,
            )
            .await
            .expect("seed corrupt authority");
        assert!(
            matches!(
                RuntimeDeliveryInbox::new(store)
                    .list_pending(&runtime_id, 1)
                    .await,
                Err(RuntimeDeliveryError::Corrupt(_))
            ),
            "corruption {mutation} must not erase successful recipient custody"
        );
    }
}

#[tokio::test]
async fn tampered_group_summary_cannot_describe_refused_recipients_as_applied() {
    let store = Arc::new(InMemoryRuntimeStore::new());
    let runtime_id = LogicalRuntimeId::new("rt:recipient-summary");
    partially_settle_recipients(store.clone(), &runtime_id).await;
    finish_reopened_recipients(store.clone(), &runtime_id).await;
    let observed = store
        .load_runtime_delivery_authority(&runtime_id)
        .await
        .expect("read")
        .expect("authority");
    let mut json: serde_json::Value = serde_json::from_slice(observed.state_json()).expect("JSON");
    json["state"]["recipients"]["groups"]["group"] = "all_applied".into();
    store
        .compare_and_swap_runtime_delivery_authority(
            &runtime_id,
            Some(observed.revision()),
            RuntimeDeliveryAuthorityRecord::from_parts(
                observed.revision() + 1,
                serde_json::to_vec(&json).expect("JSON"),
            ),
            None,
        )
        .await
        .expect("seed false summary");
    assert!(matches!(
        RuntimeDeliveryInbox::new(store)
            .delivery_status(&runtime_id, &RuntimeDeliveryId::new("group").expect("id"))
            .await,
        Err(RuntimeDeliveryError::Corrupt(_))
    ));
}
