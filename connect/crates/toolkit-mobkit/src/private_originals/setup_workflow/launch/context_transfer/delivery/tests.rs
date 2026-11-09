//! Actual task-custody mechanics only. This control claims no native admission,
//! completed transfer, store nonapplication or recipient authorization.
use super::*;
use futures::FutureExt;
use std::{panic::AssertUnwindSafe, time::Duration};

#[tokio::test]
async fn cancelled_delivery_observer_retains_and_joins_the_same_task() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let ended = Arc::new(AtomicBool::new(false));
    let task_entered = entered.clone();
    let task_release = release.clone();
    let task_ended = ended.clone();
    let started = entered.notified();
    tokio::pin!(started);
    started.as_mut().enable();
    let task = tokio::spawn(async move {
        task_entered.notify_one();
        task_release.notified().await;
        task_ended.store(true, Ordering::Release);
        Ok(WorkflowDeliveryObservation::Pending)
    });
    let run = Arc::new(DeliveryRun {
        join: tokio::sync::Mutex::new(DeliveryJoin::Running(task)),
        native: Arc::new(Mutex::new(None)),
        joined: AtomicBool::new(false),
    });
    let checked = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(1), started)
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), run.join())
                .await
                .is_err()
        );
        assert!(!run.joined.load(Ordering::Acquire));
        assert!(!ended.load(Ordering::Acquire));
        assert!(!run.finished());
    })
    .catch_unwind()
    .await;
    // Release and join even when the observer assertion panics. No test exit
    // drops an entered child and pretends the cancellation joined it.
    release.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(1), run.join())
        .await
        .unwrap()
        .unwrap();
    assert!(ended.load(Ordering::Acquire));
    assert!(run.joined.load(Ordering::Acquire));
    assert!(run.finished());
    assert_eq!(outcome, WorkflowDeliveryObservation::Pending);
    assert_eq!(run.join().await.unwrap(), outcome);
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}

/// The absence witness is controlled at the native-read boundary; this test
/// does not manufacture native nonadmission. It uses the production retained
/// run registrar and actual SQLite claims to prove that a delayed old observer
/// cannot overlap a newer claim, including after that newer lease expires.
#[tokio::test]
async fn old_absence_observer_cannot_reset_a_newer_delivery_claim() {
    use toolkit_store::{AdmissionRequest, EffectKey, SqliteStore, StoreKey, ToolkitStore};
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(directory.path().join("delivery.sqlite")).unwrap());
    let record = StoreKey {
        realm: "delivery-test".into(),
        owner: "slot".into(),
        id: "one".into(),
    };
    let data = RecordData {
        schema: "test.delivery-order".into(),
        version: 1,
        body: serde_json::json!({"controlled_absence":true}),
    };
    store
        .admit(AdmissionRequest {
            key: record.clone(),
            admission_key: "delivery-one".into(),
            data: data.clone(),
            events: vec![],
            effects: vec![EffectIntent {
                id: "input".into(),
                kind: "test.delivery".into(),
                request: data.clone(),
                idempotency_key: "one-input".into(),
                replay_safety: ReplaySafety::ReconcileBeforeRetry,
            }],
        })
        .await
        .unwrap();
    let key = EffectKey {
        record,
        id: "input".into(),
    };
    let EffectClaimOutcome::Claimed(first) =
        store.claim_effect(&key, 1, "first", 60_000).await.unwrap()
    else {
        panic!("first claim")
    };
    store
        .acknowledge_effect(
            &first.lease.unwrap(),
            EffectOutcome::Uncertain {
                evidence: data.clone(),
            },
        )
        .await
        .unwrap();
    let absence_read = Arc::new(tokio::sync::Notify::new());
    let release_old = Arc::new(tokio::sync::Notify::new());
    let retry_claimed = Arc::new(tokio::sync::Notify::new());
    let release_retry = Arc::new(tokio::sync::Notify::new());
    let retry_ready = Arc::new(AtomicBool::new(false));
    let starts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let current = Mutex::new(None);
    let old = {
        let store = store.clone();
        let key = key.clone();
        let data = data.clone();
        let read = absence_read.clone();
        let release = release_old.clone();
        let ready = retry_ready.clone();
        DeliveryRun::retain_or_start(&mut current.lock().unwrap(), move |_| async move {
            let effect = store.get_effect(&key).await.unwrap().unwrap();
            assert_eq!(effect.attempt, 1);
            assert_eq!(effect.status, EffectStatus::Uncertain);
            read.notify_one();
            release.notified().await;
            let claim = store
                .claim_reconciliation(&key, "old-observer", 60_000)
                .await
                .unwrap();
            assert_eq!(claim.attempt, 1);
            store
                .reconcile_effect(
                    &claim.lease.unwrap(),
                    Reconciliation::NotApplied { evidence: data },
                )
                .await
                .unwrap();
            ready.store(true, Ordering::Release);
            Ok(WorkflowDeliveryObservation::NotApplied)
        })
    };
    let checked = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(1), absence_read.notified())
            .await
            .unwrap();
        let calls = starts.clone();
        let competing = DeliveryRun::retain_or_start(&mut current.lock().unwrap(), move |_| {
            calls.fetch_add(1, Ordering::AcqRel);
            async { Ok(WorkflowDeliveryObservation::Pending) }
        });
        assert!(Arc::ptr_eq(&competing, &old));
        assert_eq!(
            starts.load(Ordering::Acquire),
            0,
            "no retry starts behind an unjoined absence read"
        );
        assert_eq!(store.get_effect(&key).await.unwrap().unwrap().attempt, 1);
        assert!(!retry_ready.load(Ordering::Acquire));
        release_old.notify_one();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), old.join())
                .await
                .unwrap()
                .unwrap(),
            WorkflowDeliveryObservation::NotApplied
        );
        assert!(retry_ready.load(Ordering::Acquire));
        let retry = {
            let store = store.clone();
            let key = key.clone();
            let calls = starts.clone();
            let ready = retry_ready.clone();
            let claimed = retry_claimed.clone();
            let release = release_retry.clone();
            DeliveryRun::retain_or_start(&mut current.lock().unwrap(), move |_| async move {
                calls.fetch_add(1, Ordering::AcqRel);
                ready.store(false, Ordering::Release);
                let EffectClaimOutcome::Claimed(claim) =
                    store.claim_effect(&key, 1, "retry", 1).await.unwrap()
                else {
                    panic!("new attempt after joined nonapplication")
                };
                assert_eq!(claim.attempt, 2);
                claimed.notify_one();
                release.notified().await;
                Ok(WorkflowDeliveryObservation::Pending)
            })
        };
        assert!(!Arc::ptr_eq(&retry, &old));
        tokio::time::timeout(Duration::from_secs(1), retry_claimed.notified())
            .await
            .unwrap();
        let newer = store.get_effect(&key).await.unwrap().unwrap();
        let expiry = newer.lease.as_ref().unwrap().expires_at_ms;
        tokio::time::timeout(Duration::from_secs(1), async {
            while store.now_ms().await.unwrap() <= expiry {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            old.join().await.unwrap(),
            WorkflowDeliveryObservation::NotApplied
        );
        assert!(
            !retry_ready.load(Ordering::Acquire),
            "joined old result cannot reset new attempt readiness"
        );
        assert_eq!(
            store.get_effect(&key).await.unwrap().unwrap(),
            newer,
            "expired newer claim cannot be reconciled by the old witness"
        );
        let calls = starts.clone();
        let third = DeliveryRun::retain_or_start(&mut current.lock().unwrap(), move |_| {
            calls.fetch_add(1, Ordering::AcqRel);
            async { Ok(WorkflowDeliveryObservation::Pending) }
        });
        assert!(Arc::ptr_eq(&third, &retry));
        assert_eq!(starts.load(Ordering::Acquire), 1);
    })
    .catch_unwind()
    .await;
    // All real tasks remain owned and are joined on both success and panic.
    release_old.notify_one();
    release_retry.notify_one();
    let latest = current.lock().unwrap().clone();
    let _ = tokio::time::timeout(Duration::from_secs(1), old.join())
        .await
        .unwrap();
    if let Some(latest) = latest {
        let _ = tokio::time::timeout(Duration::from_secs(1), latest.join())
            .await
            .unwrap();
    }
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}

/// The actual pre-dispatch store observation lives inside the outer advance,
/// before an inner delivery slot or native input is required. No recipient or
/// native admission is fabricated by this task/store ownership control.
#[tokio::test]
async fn slow_advance_observation_survives_two_polls_without_blocking_another() {
    use crate::installed_host::acquisition::recovery::AcquisitionRecovery;
    use std::sync::atomic::AtomicUsize;
    use toolkit_store::{AdmissionRequest, SqliteStore, StoreKey, ToolkitStore};
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(directory.path().join("advance.sqlite")).unwrap());
    let key = StoreKey {
        realm: "advance-test".into(),
        owner: "observation".into(),
        id: "one".into(),
    };
    let data = RecordData {
        schema: "test.advance-observation".into(),
        version: 1,
        body: serde_json::json!({"retained":true}),
    };
    store
        .admit(AdmissionRequest {
            key: key.clone(),
            admission_key: "one".into(),
            data: data.clone(),
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();
    let lifecycle = AcquisitionRecovery::default();
    let control = lifecycle.transfer_control();
    let current = Mutex::new(None);
    let competing = Mutex::new(None);
    let entered = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let finished = Arc::new(AtomicUsize::new(0));
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let first = {
        let store = store.clone();
        let key = key.clone();
        let data = data.clone();
        let entered = entered.clone();
        let reads = reads.clone();
        let finished = finished.clone();
        let reached = reached.clone();
        let release = release.clone();
        DeliveryRun::register_advance(&control, &current, move || async move {
            entered.fetch_add(1, Ordering::AcqRel);
            let row = store.get(&key).await.unwrap().unwrap();
            assert_eq!(row.data, data);
            reads.fetch_add(1, Ordering::AcqRel);
            reached.notify_one();
            // Hold the completed canonical observation before delivery entry.
            release.notified().await;
            finished.fetch_add(1, Ordering::AcqRel);
            Ok(WorkflowDeliveryObservation::Pending)
        })
        .unwrap()
    };
    let checked = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(1), reached.notified())
            .await
            .unwrap();
        for _ in 0..2 {
            let later = DeliveryRun::register_advance(&control, &current, || async {
                panic!("a later poll restarted the pre-delivery observation")
            })
            .unwrap();
            assert!(Arc::ptr_eq(&first, &later));
            assert!(
                tokio::time::timeout(Duration::from_millis(10), later.join())
                    .await
                    .is_err()
            );
            assert_eq!(entered.load(Ordering::Acquire), 1);
            assert_eq!(reads.load(Ordering::Acquire), 1);
            assert_eq!(finished.load(Ordering::Acquire), 0);
        }
        let other = {
            let store = store.clone();
            let key = key.clone();
            let data = data.clone();
            DeliveryRun::register_advance(&control, &competing, move || async move {
                assert_eq!(store.get(&key).await.unwrap().unwrap().data, data);
                Err(PrivateOriginalError::Conflict)
            })
            .unwrap()
        };
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), other.join())
                .await
                .unwrap(),
            Err(PrivateOriginalError::Conflict),
            "another observation and its typed result are independent"
        );
        assert_eq!(finished.load(Ordering::Acquire), 0);
        lifecycle.stop();
        assert!(matches!(
            DeliveryRun::register_advance(&control, &Mutex::new(None), || async {
                panic!("registration after stop")
            }),
            Err(PrivateOriginalError::Unavailable)
        ));
    })
    .catch_unwind()
    .await;
    // Retain and join both real task owners even when any assertion fails.
    lifecycle.stop();
    release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), first.join())
            .await
            .unwrap()
            .unwrap(),
        WorkflowDeliveryObservation::Pending
    );
    let other = competing.lock().unwrap().clone();
    if let Some(other) = other {
        let _ = tokio::time::timeout(Duration::from_secs(1), other.join())
            .await
            .unwrap();
    }
    assert_eq!(entered.load(Ordering::Acquire), 1);
    assert_eq!(reads.load(Ordering::Acquire), 1);
    assert_eq!(finished.load(Ordering::Acquire), 1);
    lifecycle.join().await.unwrap();
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}

/// Native observations are controlled at this boundary. The real SQLite owner
/// advances two attempts and issues the actual reconciliation leases. This
/// proves effect-witness ordering, not native admission or cold reconstruction.
#[tokio::test]
async fn delayed_delivery_observation_cannot_acknowledge_a_newer_attempt() {
    use toolkit_store::{AdmissionRequest, EffectKey, SqliteStore, StoreKey, ToolkitStore};
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("accepted-attempt.sqlite")).unwrap();
    let record = StoreKey {
        realm: "delivery-test".into(),
        owner: "accepted".into(),
        id: "one".into(),
    };
    let data = RecordData {
        schema: "test.delivery-observation".into(),
        version: 1,
        body: serde_json::json!({"cancelled":false}),
    };
    let evidence = RecordData {
        schema: "test.controlled-native-observation".into(),
        version: 1,
        body: serde_json::json!({"boundary":"controlled"}),
    };
    store
        .admit(AdmissionRequest {
            key: record.clone(),
            admission_key: "one".into(),
            data: data.clone(),
            events: vec![],
            effects: vec![EffectIntent {
                id: "input".into(),
                kind: "test.delivery".into(),
                request: data,
                idempotency_key: "exact-input".into(),
                replay_safety: ReplaySafety::ReconcileBeforeRetry,
            }],
        })
        .await
        .unwrap();
    let key = EffectKey {
        record: record.clone(),
        id: "input".into(),
    };
    let EffectClaimOutcome::Claimed(first) =
        store.claim_effect(&key, 1, "first", 60_000).await.unwrap()
    else {
        panic!("first dispatch claim")
    };
    store
        .acknowledge_effect(
            &first.lease.unwrap(),
            EffectOutcome::Uncertain {
                evidence: evidence.clone(),
            },
        )
        .await
        .unwrap();
    let delayed = store.get_effect(&key).await.unwrap().unwrap();
    assert_eq!(delayed.attempt, 1);

    // While an earlier observer is suspended, another owner completes the
    // previous attempt's controlled nonapplication and starts the next one.
    let joined = store
        .claim_reconciliation(&key, "joined-first", 60_000)
        .await
        .unwrap();
    store
        .reconcile_effect(
            exact_reconciliation_lease(&delayed, &joined).unwrap(),
            Reconciliation::NotApplied {
                evidence: evidence.clone(),
            },
        )
        .await
        .unwrap();
    let EffectClaimOutcome::Claimed(second) =
        store.claim_effect(&key, 1, "second", 60_000).await.unwrap()
    else {
        panic!("second dispatch claim")
    };
    assert_eq!(second.attempt, 2);
    store
        .acknowledge_effect(
            &second.lease.unwrap(),
            EffectOutcome::Uncertain {
                evidence: evidence.clone(),
            },
        )
        .await
        .unwrap();
    let current = store.get_effect(&key).await.unwrap().unwrap();

    // Cancellation updates the canonical owner row. It does not erase a real
    // old admission or forbid metadata acknowledgment for the current attempt.
    let row = store.get(&record).await.unwrap().unwrap();
    let mut cancelled = row.data.clone();
    cancelled.body["cancelled"] = serde_json::json!(true);
    store
        .compare_and_swap(Transition {
            key: record.clone(),
            expected_revision: row.revision,
            expected_digest: Some(row.content_digest),
            claim: None,
            data: cancelled.clone(),
            events: vec![],
            effects: vec![],
        })
        .await
        .unwrap();

    let claimed = store
        .claim_reconciliation(&key, "delayed-observer", 60_000)
        .await
        .unwrap();
    assert_eq!(claimed.attempt, 2);
    assert!(matches!(
        exact_reconciliation_lease(&delayed, &claimed),
        Err(PrivateOriginalError::Conflict)
    ));
    assert_eq!(
        store.get_effect(&key).await.unwrap().unwrap(),
        claimed,
        "a rejected old observation does not settle or reset the current attempt"
    );
    let lease = exact_reconciliation_lease(&current, &claimed).unwrap();
    assert_eq!(lease.kind, EffectClaimKind::Reconcile);
    let applied = store
        .reconcile_effect(
            lease,
            Reconciliation::Applied {
                receipt: evidence.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(applied.status, EffectStatus::Succeeded);
    assert_eq!(applied.attempt, 2);
    assert_eq!(
        applied.outcome,
        Some(EffectOutcome::Succeeded { receipt: evidence })
    );
    assert!(applied.lease.is_none());
    assert_eq!(store.get(&record).await.unwrap().unwrap().data, cancelled);
    assert!(matches!(
        store.claim_reconciliation(&key, "repeat", 60_000).await,
        Err(StoreError::EffectState)
    ));
    assert_eq!(
        store.get_effect(&key).await.unwrap().unwrap(),
        applied,
        "historical acknowledgment creates neither a new claim nor another dispatch attempt"
    );
}

/// The native decision is controlled at the joined-task boundary. Actual store
/// claims prove that losing or forgetting a claim cannot manufacture ownership
/// of the effect that another executor may still apply.
#[tokio::test]
async fn unowned_or_old_delivery_claim_never_proves_nonadmission() {
    use toolkit_store::{AdmissionRequest, EffectKey, SqliteStore, StoreKey, ToolkitStore};
    fn native() -> Arc<DeliveryNative> {
        Arc::new(DeliveryNative {
            claim: OnceLock::new(),
            not_applied: OnceLock::new(),
            decision: Mutex::new(NativeDecision::NeverSubmitted),
            joined: AtomicBool::new(false),
            custody: Arc::new(DeliveryCustody::default()),
        })
    }
    async fn join(native: &Arc<DeliveryNative>) {
        let selected = native.clone();
        let run = DeliveryRun::retain_or_start(&mut None, move |held| async move {
            *held.lock().unwrap() = Some(selected);
            Ok(WorkflowDeliveryObservation::Pending)
        });
        assert_eq!(
            run.join().await.unwrap(),
            WorkflowDeliveryObservation::Pending
        );
        assert!(native.joined.load(Ordering::Acquire));
    }
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("claim-ownership.sqlite")).unwrap();
    let record = StoreKey {
        realm: "delivery-test".into(),
        owner: "claim-owner".into(),
        id: "one".into(),
    };
    let data = RecordData {
        schema: "test.claim-owner".into(),
        version: 1,
        body: serde_json::json!({"native_boundary":"controlled"}),
    };
    let intent = |id: &str| EffectIntent {
        id: id.into(),
        kind: "test.delivery".into(),
        request: data.clone(),
        idempotency_key: id.into(),
        replay_safety: ReplaySafety::ReconcileBeforeRetry,
    };
    store
        .admit(AdmissionRequest {
            key: record.clone(),
            admission_key: "one".into(),
            data: data.clone(),
            events: vec![],
            effects: vec![intent("foreign"), intent("lost"), intent("older")],
        })
        .await
        .unwrap();
    let key = |id: &str| EffectKey {
        record: record.clone(),
        id: id.into(),
    };

    let foreign_key = key("foreign");
    let losing = native();
    let EffectClaimOutcome::Claimed(foreign) = store
        .claim_effect(&foreign_key, 1, "other-owner", 60_000)
        .await
        .unwrap()
    else {
        panic!("other owner claim")
    };
    assert!(matches!(
        store
            .claim_effect(&foreign_key, 1, "losing-owner", 60_000)
            .await,
        Err(StoreError::Claimed)
    ));
    join(&losing).await;
    assert!(!losing.can_reconcile_absence_for(&foreign).unwrap());
    store
        .acknowledge_effect(
            &foreign.lease.unwrap(),
            EffectOutcome::Uncertain {
                evidence: data.clone(),
            },
        )
        .await
        .unwrap();
    let EffectClaimOutcome::NeedsReconciliation(uncertain) = store
        .claim_effect(&foreign_key, 1, "losing-owner", 60_000)
        .await
        .unwrap()
    else {
        panic!("existing foreign attempt needs reconciliation")
    };
    assert!(!losing.can_reconcile_absence_for(&uncertain).unwrap());
    assert_eq!(
        store.get_effect(&foreign_key).await.unwrap().unwrap(),
        uncertain
    );

    let lost_key = key("lost");
    let lost = native();
    // A committed claim whose return was lost is not an observed owned claim.
    // Drop the actual reply without calling the production capture boundary.
    drop(
        store
            .claim_effect(&lost_key, 1, "lost-reply", 60_000)
            .await
            .unwrap(),
    );
    join(&lost).await;
    let lost_effect = store.get_effect(&lost_key).await.unwrap().unwrap();
    assert_eq!(lost_effect.status, EffectStatus::InFlight);
    assert!(!lost.can_reconcile_absence_for(&lost_effect).unwrap());
    assert_eq!(
        store.get_effect(&lost_key).await.unwrap().unwrap(),
        lost_effect
    );

    let older_key = key("older");
    let older = native();
    let pending = store.get_effect(&older_key).await.unwrap().unwrap();
    let EffectClaimOutcome::Claimed(first) = store
        .claim_effect(&older_key, 1, "owned-first", 60_000)
        .await
        .unwrap()
    else {
        panic!("owned first claim")
    };
    let lease = older.record_claim(&pending, first).unwrap();
    let owned = store
        .acknowledge_effect(
            &lease,
            EffectOutcome::Uncertain {
                evidence: data.clone(),
            },
        )
        .await
        .unwrap();
    assert!(
        !older.can_reconcile_absence_for(&owned).unwrap(),
        "an entered task must join first"
    );
    join(&older).await;
    assert!(older.can_reconcile_absence_for(&owned).unwrap());
    let first_observer = store
        .claim_reconciliation(&older_key, "first-observer", 60_000)
        .await
        .unwrap();
    store
        .reconcile_effect(
            exact_reconciliation_lease(&owned, &first_observer).unwrap(),
            Reconciliation::NotApplied { evidence: data },
        )
        .await
        .unwrap();
    let EffectClaimOutcome::Claimed(newer) = store
        .claim_effect(&older_key, 1, "new-owner", 60_000)
        .await
        .unwrap()
    else {
        panic!("newer owner claim")
    };
    assert_eq!(newer.attempt, owned.attempt + 1);
    assert!(
        !older.can_reconcile_absence_for(&newer).unwrap(),
        "old joined absence does not cover a newer attempt"
    );
    assert_eq!(store.get_effect(&older_key).await.unwrap().unwrap(), newer);
}

/// The actual SQLite record CAS models a later waiter joining the workflow:
/// it advances the owner revision and adds a separate delivery obligation.
/// This exercises the production claim/retry state and retained task ordering.
/// Native nonadmission is controlled; no workflow or native authority is created.
#[tokio::test]
async fn delivery_retry_survives_only_definite_revision_conflict() {
    use toolkit_store::{AdmissionRequest, EffectKey, SqliteStore, StoreKey, ToolkitStore};
    let directory = tempfile::tempdir().unwrap();
    let store =
        Arc::new(SqliteStore::open(directory.path().join("retry-conflict.sqlite")).unwrap());
    for foreign in [false, true] {
        let record = StoreKey {
            realm: "delivery-test".into(),
            owner: "retry-conflict".into(),
            id: (if foreign { "foreign" } else { "retry" }).into(),
        };
        let data = RecordData {
            schema: "test.workflow-join-cas".into(),
            version: 1,
            body: serde_json::json!({"waiters":["a"]}),
        };
        let intent = EffectIntent {
            id: "a-input".into(),
            kind: "test.delivery".into(),
            request: data.clone(),
            idempotency_key: format!("same-retained-input-{}", record.id),
            replay_safety: ReplaySafety::ReconcileBeforeRetry,
        };
        store
            .admit(AdmissionRequest {
                key: record.clone(),
                admission_key: format!("workflow-{}", record.id),
                data: data.clone(),
                events: vec![],
                effects: vec![intent.clone()],
            })
            .await
            .unwrap();
        let key = EffectKey {
            record: record.clone(),
            id: intent.id.clone(),
        };
        let retained = Arc::new(Mutex::new(None));
        let current = Mutex::new(None);
        let first = {
            let store = store.clone();
            let key = key.clone();
            let retained = retained.clone();
            let data = data.clone();
            DeliveryRun::retain_or_start(&mut current.lock().unwrap(), move |owned| async move {
                let effect = store.get_effect(&key).await.unwrap().unwrap();
                let Some((_, lease)) =
                    claim_delivery_effect(store.as_ref(), &effect, 1, &retained, &owned)
                        .await
                        .unwrap()
                else {
                    panic!("initial actual claim")
                };
                // Stop before native submission, then settle the actual task.
                store
                    .acknowledge_effect(&lease, EffectOutcome::Uncertain { evidence: data })
                    .await
                    .unwrap();
                Ok(WorkflowDeliveryObservation::Pending)
            })
        };
        first.join().await.unwrap();
        let original_native = retained.lock().unwrap().clone().unwrap();
        let uncertain = store.get_effect(&key).await.unwrap().unwrap();
        assert!(
            original_native
                .can_reconcile_absence_for(&uncertain)
                .unwrap()
        );
        let reconciliation = store
            .claim_reconciliation(&key, "controlled-absence", 60_000)
            .await
            .unwrap();
        let pending = store
            .reconcile_effect(
                exact_reconciliation_lease(&uncertain, &reconciliation).unwrap(),
                Reconciliation::NotApplied {
                    evidence: data.clone(),
                },
            )
            .await
            .unwrap();
        original_native.record_not_applied(pending.clone()).unwrap();
        assert!(delivery_claim_ready(&retained, &pending).unwrap());

        let read = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let stale_revision = Arc::new(OnceLock::new());
        let retry = {
            let store = store.clone();
            let key = key.clone();
            let retained = retained.clone();
            let read = read.clone();
            let release = release.clone();
            let revision = stale_revision.clone();
            DeliveryRun::retain_or_start(&mut current.lock().unwrap(), move |owned| async move {
                let effect = store.get_effect(&key).await.unwrap().unwrap();
                let row = store.get(&key.record).await.unwrap().unwrap();
                revision.set(row.revision).unwrap();
                read.notify_one();
                release.notified().await;
                assert!(
                    claim_delivery_effect(store.as_ref(), &effect, row.revision, &retained, &owned)
                        .await
                        .unwrap()
                        .is_none(),
                    "a real stale revision cannot acquire a dispatch claim"
                );
                let attempted = owned.lock().unwrap().clone().unwrap();
                assert!(attempted.claim.get().is_none());
                assert!(attempted.not_applied.get().is_none());
                Ok(WorkflowDeliveryObservation::Pending)
            })
        };
        let checked = AssertUnwindSafe(async {
            tokio::time::timeout(Duration::from_secs(1), read.notified())
                .await
                .unwrap();
            let row = store.get(&record).await.unwrap().unwrap();
            assert_eq!(Some(&row.revision), stale_revision.get());
            let mut joined = row.data.clone();
            joined.body["waiters"] = serde_json::json!(["a", "b"]);
            let other = EffectIntent {
                id: "b-input".into(),
                kind: "test.delivery".into(),
                request: joined.clone(),
                idempotency_key: format!("independent-b-input-{}", record.id),
                replay_safety: ReplaySafety::ReconcileBeforeRetry,
            };
            let joined = store
                .compare_and_swap(Transition {
                    key: record.clone(),
                    expected_revision: row.revision,
                    expected_digest: Some(row.content_digest),
                    claim: None,
                    data: joined,
                    events: vec![],
                    effects: vec![other.clone()],
                })
                .await
                .unwrap();
            assert_eq!(joined.revision, row.revision + 1);
            assert_eq!(
                store.get_effect(&key).await.unwrap().unwrap(),
                pending,
                "a waiter record change preserves the existing delivery effect"
            );
            assert_eq!(
                store
                    .get_effect(&EffectKey {
                        record: record.clone(),
                        id: other.id.clone()
                    })
                    .await
                    .unwrap()
                    .unwrap()
                    .intent,
                other
            );
            release.notify_one();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(1), retry.join())
                    .await
                    .unwrap()
                    .unwrap(),
                WorkflowDeliveryObservation::Pending
            );
            assert!(
                Arc::ptr_eq(retained.lock().unwrap().as_ref().unwrap(), &original_native),
                "definite pre-claim Conflict must preserve the exact successful NotApplied witness"
            );
            assert_eq!(store.get_effect(&key).await.unwrap().unwrap(), pending);
            assert!(delivery_claim_ready(&retained, &pending).unwrap());
            if foreign {
                // Another executor advances the effect after the conflict.
                // Even its subsequent NotApplied cannot extend our old proof.
                let EffectClaimOutcome::Claimed(newer) = store
                    .claim_effect(&key, joined.revision, "other-executor", 60_000)
                    .await
                    .unwrap()
                else {
                    panic!("foreign newer claim")
                };
                store
                    .acknowledge_effect(
                        &newer.lease.unwrap(),
                        EffectOutcome::Uncertain {
                            evidence: data.clone(),
                        },
                    )
                    .await
                    .unwrap();
                let observed = store.get_effect(&key).await.unwrap().unwrap();
                let reconcile = store
                    .claim_reconciliation(&key, "foreign-observer", 60_000)
                    .await
                    .unwrap();
                let newer_pending = store
                    .reconcile_effect(
                        exact_reconciliation_lease(&observed, &reconcile).unwrap(),
                        Reconciliation::NotApplied {
                            evidence: data.clone(),
                        },
                    )
                    .await
                    .unwrap();
                assert_eq!(newer_pending.attempt, pending.attempt + 1);
                assert!(!delivery_claim_ready(&retained, &newer_pending).unwrap());
                let blocked = {
                    let store = store.clone();
                    let key = key.clone();
                    let retained = retained.clone();
                    DeliveryRun::retain_or_start(
                        &mut current.lock().unwrap(),
                        move |owned| async move {
                            let effect = store.get_effect(&key).await.unwrap().unwrap();
                            assert!(
                                claim_delivery_effect(
                                    store.as_ref(),
                                    &effect,
                                    joined.revision,
                                    &retained,
                                    &owned
                                )
                                .await
                                .unwrap()
                                .is_none()
                            );
                            assert!(
                                owned.lock().unwrap().is_none(),
                                "ineligible foreign attempt never reaches claim"
                            );
                            Ok(WorkflowDeliveryObservation::Pending)
                        },
                    )
                };
                blocked.join().await.unwrap();
                assert_eq!(
                    store.get_effect(&key).await.unwrap().unwrap(),
                    newer_pending
                );
            } else {
                let successful = {
                    let store = store.clone();
                    let key = key.clone();
                    let retained = retained.clone();
                    DeliveryRun::retain_or_start(
                        &mut current.lock().unwrap(),
                        move |owned| async move {
                            let effect = store.get_effect(&key).await.unwrap().unwrap();
                            let Some((native, lease)) = claim_delivery_effect(
                                store.as_ref(),
                                &effect,
                                joined.revision,
                                &retained,
                                &owned,
                            )
                            .await
                            .unwrap() else {
                                panic!("the later current revision must permit the exact retry")
                            };
                            let actual = native.claim.get().unwrap();
                            assert_eq!(actual.attempt, effect.attempt + 1);
                            assert_eq!(actual.key, effect.key);
                            assert_eq!(actual.intent, effect.intent);
                            assert_eq!(actual.request_digest, effect.request_digest);
                            assert_eq!(actual.lease.as_ref(), Some(&lease));
                            assert!(native.not_applied.get().is_none());
                            Ok(WorkflowDeliveryObservation::Pending)
                        },
                    )
                };
                successful.join().await.unwrap();
                let current_effect = store.get_effect(&key).await.unwrap().unwrap();
                assert_eq!(current_effect.attempt, pending.attempt + 1);
                assert_eq!(current_effect.status, EffectStatus::InFlight);
                assert!(!Arc::ptr_eq(
                    retained.lock().unwrap().as_ref().unwrap(),
                    &original_native
                ));
                assert!(!delivery_claim_ready(&retained, &current_effect).unwrap());
            }
        })
        .catch_unwind()
        .await;
        release.notify_one();
        let latest = current.lock().unwrap().clone();
        let _ = tokio::time::timeout(Duration::from_secs(1), retry.join())
            .await
            .unwrap();
        if let Some(latest) = latest {
            let _ = tokio::time::timeout(Duration::from_secs(1), latest.join())
                .await
                .unwrap();
        }
        if let Err(panic) = checked {
            std::panic::resume_unwind(panic);
        }
    }
}

/// RetrySafe is a different canonical resolution. Even an exact joined native
/// nonadmission witness cannot turn that generic Pending row into our observed
/// successful NotApplied acknowledgment.
#[tokio::test]
async fn retry_safe_does_not_manufacture_delivery_retry_evidence() {
    use toolkit_store::{AdmissionRequest, EffectKey, SqliteStore, StoreKey, ToolkitStore};
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteStore::open(directory.path().join("retry-safe.sqlite")).unwrap());
    let record = StoreKey {
        realm: "delivery-test".into(),
        owner: "retry-safe".into(),
        id: "one".into(),
    };
    let data = RecordData {
        schema: "test.retry-safe".into(),
        version: 1,
        body: serde_json::json!({"controlled":true}),
    };
    store
        .admit(AdmissionRequest {
            key: record.clone(),
            admission_key: "one".into(),
            data: data.clone(),
            events: vec![],
            effects: vec![EffectIntent {
                id: "input".into(),
                kind: "test.delivery".into(),
                request: data.clone(),
                idempotency_key: "same-input".into(),
                replay_safety: ReplaySafety::ReconcileBeforeRetry,
            }],
        })
        .await
        .unwrap();
    let key = EffectKey {
        record,
        id: "input".into(),
    };
    let retained = Arc::new(Mutex::new(None));
    let run = {
        let store = store.clone();
        let key = key.clone();
        let retained = retained.clone();
        let data = data.clone();
        DeliveryRun::retain_or_start(&mut None, move |owned| async move {
            let effect = store.get_effect(&key).await.unwrap().unwrap();
            let Some((_, lease)) =
                claim_delivery_effect(store.as_ref(), &effect, 1, &retained, &owned)
                    .await
                    .unwrap()
            else {
                panic!("actual first claim")
            };
            store
                .acknowledge_effect(&lease, EffectOutcome::Uncertain { evidence: data })
                .await
                .unwrap();
            Ok(WorkflowDeliveryObservation::Pending)
        })
    };
    run.join().await.unwrap();
    let native = retained.lock().unwrap().clone().unwrap();
    let uncertain = store.get_effect(&key).await.unwrap().unwrap();
    assert!(native.can_reconcile_absence_for(&uncertain).unwrap());
    let claim = store
        .claim_reconciliation(&key, "retry-safe-owner", 60_000)
        .await
        .unwrap();
    let pending = store
        .reconcile_effect(
            exact_reconciliation_lease(&uncertain, &claim).unwrap(),
            Reconciliation::RetrySafe { evidence: data },
        )
        .await
        .unwrap();
    assert_eq!(pending.status, EffectStatus::Pending);
    assert!(pending.attempt > 0);
    assert!(native.not_applied.get().is_none());
    assert!(!delivery_claim_ready(&retained, &pending).unwrap());
    assert!(
        claim_delivery_effect(store.as_ref(), &pending, 1, &retained, &Mutex::new(None))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.get_effect(&key).await.unwrap().unwrap(), pending);
    assert!(Arc::ptr_eq(
        retained.lock().unwrap().as_ref().unwrap(),
        &native
    ));
}
