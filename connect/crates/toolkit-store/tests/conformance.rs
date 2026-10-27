use serde_json::json;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use tempfile::TempDir;
use toolkit_store::*;

fn data(value: u64) -> RecordData {
    RecordData {
        schema: "test.operation".into(),
        version: 1,
        body: json!({"value": value}),
    }
}

fn key(id: &str) -> StoreKey {
    StoreKey {
        realm: "test-realm".into(),
        owner: "test-owner".into(),
        id: id.into(),
    }
}

fn admission(id: &str) -> AdmissionRequest {
    AdmissionRequest {
        key: key(id),
        admission_key: format!("admission-{id}"),
        data: data(1),
        events: vec![EventWrite {
            kind: "admitted".into(),
            data: data(1),
        }],
        effects: vec![],
    }
}

fn transition(record: &StoredRecord, value: u64) -> Transition {
    Transition {
        key: record.key.clone(),
        expected_revision: record.revision,
        expected_digest: Some(record.content_digest.clone()),
        claim: None,
        data: data(value),
        events: vec![EventWrite {
            kind: "changed".into(),
            data: data(value),
        }],
        effects: vec![],
    }
}

fn effect(id: &str, safety: ReplaySafety) -> EffectIntent {
    EffectIntent {
        id: id.into(),
        kind: "test.delivery".into(),
        request: data(19),
        idempotency_key: format!("idempotency-{id}"),
        replay_safety: safety,
    }
}

fn fixture() -> (TempDir, SqliteStore) {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    (directory, store)
}

async fn wait_past(store: &SqliteStore, timestamp: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while store.now_ms().await.unwrap() <= timestamp {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("store clock advanced");
}

#[tokio::test]
async fn durable_reopen_keeps_admission_cas_events_and_intents() {
    let (directory, store) = fixture();
    store.capabilities().require_durable_contract().unwrap();
    let mut request = admission("recover");
    request
        .effects
        .push(effect("prepare", ReplaySafety::Idempotent));
    let admitted = store.admit(request.clone()).await.unwrap();
    assert!(matches!(admitted, AdmissionOutcome::Created(_)));
    let record = store
        .compare_and_swap(transition(admitted.record(), 2))
        .await
        .unwrap();
    drop(store);
    let reopened = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    assert_eq!(
        reopened.get(&record.key).await.unwrap(),
        Some(record.clone())
    );
    let replay = reopened.admit(request).await.unwrap();
    assert!(matches!(replay, AdmissionOutcome::Existing(_)));
    assert_eq!(replay.record(), &record);
    let events = reopened.read_events(&record.key, 0, 100).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].sequence, 1);
    assert_eq!(events[1].sequence, 2);
    assert_eq!(events[1].record_revision, 2);
    let effects = reopened.list_effects(&record.key, None, 100).await.unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].status, EffectStatus::Pending);
}

#[tokio::test]
async fn admission_replay_rejects_changed_content_effects_and_record_ids() {
    let (_directory, store) = fixture();
    let request = admission("identity");
    store.admit(request.clone()).await.unwrap();
    let mut changed = request.clone();
    changed.data = data(2);
    assert!(matches!(
        store.admit(changed).await,
        Err(StoreError::AdmissionConflict)
    ));
    let mut changed = request.clone();
    changed
        .effects
        .push(effect("new", ReplaySafety::Idempotent));
    assert!(matches!(
        store.admit(changed).await,
        Err(StoreError::AdmissionConflict)
    ));
    let mut changed = request;
    changed.key.id = "another-operation".into();
    assert!(matches!(
        store.admit(changed).await,
        Err(StoreError::AdmissionConflict)
    ));
}

#[tokio::test]
async fn effect_collision_rolls_back_record_and_event_changes() {
    let (_directory, store) = fixture();
    let mut request = admission("atomic");
    request
        .effects
        .push(effect("write", ReplaySafety::ReconcileBeforeRetry));
    let record = store.admit(request).await.unwrap().record().clone();
    let mut change = transition(&record, 2);
    change
        .effects
        .push(effect("write", ReplaySafety::ReconcileBeforeRetry));
    assert!(matches!(
        store.compare_and_swap(change).await,
        Err(StoreError::EffectConflict)
    ));
    assert_eq!(store.get(&record.key).await.unwrap(), Some(record.clone()));
    assert_eq!(
        store.read_events(&record.key, 0, 100).await.unwrap().len(),
        1
    );

    let mut other = admission("other");
    other
        .effects
        .push(effect("write", ReplaySafety::ReconcileBeforeRetry));
    assert!(matches!(
        store.admit(other).await,
        Err(StoreError::EffectConflict)
    ));
    assert!(store.get(&key("other")).await.unwrap().is_none());
    assert!(
        store
            .read_events(&key("other"), 0, 100)
            .await
            .unwrap()
            .is_empty()
    );
    // The failed admission itself did not survive the rolled-back transaction.
    store.admit(admission("other")).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_connections_compete_on_revision_without_lost_updates() {
    let (directory, store) = fixture();
    let record = store
        .admit(admission("cas"))
        .await
        .unwrap()
        .record()
        .clone();
    let barrier = Arc::new(tokio::sync::Barrier::new(12));
    let mut tasks = vec![];
    for value in 2..14 {
        let contender = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
        let barrier = barrier.clone();
        let change = transition(&record, value);
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            contender.compare_and_swap(change).await
        }));
    }
    let mut winners = 0;
    let mut conflicts = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => winners += 1,
            Err(StoreError::Conflict) => conflicts += 1,
            result => panic!("unexpected CAS result: {result:?}"),
        }
    }
    assert_eq!((winners, conflicts), (1, 11));
    assert_eq!(store.get(&record.key).await.unwrap().unwrap().revision, 2);
    assert_eq!(
        store.read_events(&record.key, 0, 100).await.unwrap().len(),
        2
    );
}

#[tokio::test]
async fn lost_payload_cas_and_cross_namespace_access_are_rejected() {
    let (_directory, store) = fixture();
    let record = store
        .admit(admission("isolated"))
        .await
        .unwrap()
        .record()
        .clone();
    let mut change = transition(&record, 2);
    change.expected_digest = Some("incorrect".into());
    assert!(matches!(
        store.compare_and_swap(change).await,
        Err(StoreError::Conflict)
    ));
    let mut wrong_key = record.key.clone();
    wrong_key.owner = "other-owner".into();
    assert!(store.get(&wrong_key).await.unwrap().is_none());
    assert!(
        store
            .read_events(&wrong_key, 0, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_effects(&wrong_key, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_records(&wrong_key.scope(), None, 100)
            .await
            .unwrap()
            .is_empty()
    );
    wrong_key = record.key.clone();
    wrong_key.realm = "other-realm".into();
    assert!(store.get(&wrong_key).await.unwrap().is_none());
    assert!(
        store
            .list_records(&record.key.scope(), None, 0)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn record_claims_reject_stale_renewed_and_foreign_store_tokens() {
    let (_directory, store) = fixture();
    let record = store
        .admit(admission("lease"))
        .await
        .unwrap()
        .record()
        .clone();
    let old = store
        .claim_record(&record.key, record.revision, "first", 10)
        .await
        .unwrap();
    assert!(matches!(
        store
            .claim_record(&record.key, record.revision, "second", 1000)
            .await,
        Err(StoreError::Claimed)
    ));
    wait_past(&store, old.expires_at_ms).await;
    let fresh = store
        .claim_record(&record.key, record.revision, "second", 10_000)
        .await
        .unwrap();
    assert!(fresh.fence > old.fence);
    let mut change = transition(&record, 2);
    change.claim = Some(old.clone());
    assert!(matches!(
        store.compare_and_swap(change).await,
        Err(StoreError::Fenced)
    ));
    assert!(matches!(
        store.release_record_claim(&old).await,
        Err(StoreError::Fenced)
    ));
    let mut forged = fresh.clone();
    forged.store_id = "different-database".into();
    assert!(matches!(
        store.renew_record_claim(&forged, 1000).await,
        Err(StoreError::Fenced)
    ));
    let mut change = transition(&record, 2);
    change.claim = Some(fresh.clone());
    store.compare_and_swap(change).await.unwrap();
    store.release_record_claim(&fresh).await.unwrap();
    assert!(matches!(
        store.release_record_claim(&fresh).await,
        Err(StoreError::Fenced)
    ));
}

#[tokio::test]
async fn expired_non_idempotent_dispatch_requires_reconciliation_after_restart() {
    let (directory, store) = fixture();
    let mut request = admission("non-idempotent");
    request
        .effects
        .push(effect("send-email", ReplaySafety::ReconcileBeforeRetry));
    store.admit(request).await.unwrap();
    let key = EffectKey {
        record: key("non-idempotent"),
        id: "send-email".into(),
    };
    let EffectClaimOutcome::Claimed(claimed) = store
        .claim_effect(&key, 1, "lost-worker", 10)
        .await
        .unwrap()
    else {
        panic!("dispatch claim expected")
    };
    let stale = claimed.lease.unwrap();
    wait_past(&store, stale.expires_at_ms).await;
    drop(store);
    let reopened = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    assert!(matches!(
        reopened
            .claim_effect(&key, 1, "replacement", 1000)
            .await
            .unwrap(),
        EffectClaimOutcome::NeedsReconciliation(_)
    ));
    let receipt = EffectOutcome::Succeeded { receipt: data(99) };
    assert!(matches!(
        reopened.acknowledge_effect(&stale, receipt).await,
        Err(StoreError::Fenced)
    ));
    let reconciliation = reopened
        .claim_reconciliation(&key, "owner", 10_000)
        .await
        .unwrap();
    let lease = reconciliation.lease.unwrap();
    assert_eq!(lease.kind, EffectClaimKind::Reconcile);
    let result = reopened
        .reconcile_effect(&lease, Reconciliation::Applied { receipt: data(99) })
        .await
        .unwrap();
    assert_eq!(result.status, EffectStatus::Succeeded);
    assert_eq!(result.attempt, 1);
    assert!(matches!(
        reopened.claim_effect(&key, 1, "third", 1000).await,
        Err(StoreError::EffectState)
    ));
}

#[tokio::test]
async fn idempotent_retry_preserves_request_and_fences_late_acknowledgments() {
    let (_directory, store) = fixture();
    let mut request = admission("safe-replay");
    request
        .effects
        .push(effect("bind", ReplaySafety::Idempotent));
    store.admit(request).await.unwrap();
    let key = EffectKey {
        record: key("safe-replay"),
        id: "bind".into(),
    };
    let EffectClaimOutcome::Claimed(first) = store.claim_effect(&key, 1, "one", 10).await.unwrap()
    else {
        panic!("dispatch expected")
    };
    let stale = first.lease.clone().unwrap();
    wait_past(&store, stale.expires_at_ms).await;
    let EffectClaimOutcome::Claimed(second) =
        store.claim_effect(&key, 1, "two", 10_000).await.unwrap()
    else {
        panic!("safe replay expected")
    };
    assert_eq!(first.intent, second.intent);
    assert_eq!(first.request_digest, second.request_digest);
    assert_eq!(second.attempt, 2);
    assert!(second.lease.as_ref().unwrap().fence > stale.fence);
    assert!(matches!(
        store
            .acknowledge_effect(&stale, EffectOutcome::Succeeded { receipt: data(7) })
            .await,
        Err(StoreError::Fenced)
    ));
    let accepted = store
        .acknowledge_effect(
            &second.lease.unwrap(),
            EffectOutcome::Succeeded { receipt: data(7) },
        )
        .await
        .unwrap();
    assert_eq!(accepted.status, EffectStatus::Succeeded);
}

#[tokio::test]
async fn reconciliation_is_exclusive_and_retains_owner_retry_evidence() {
    let (_directory, store) = fixture();
    let mut request = admission("reconcile");
    request
        .effects
        .push(effect("external", ReplaySafety::ReconcileBeforeRetry));
    store.admit(request).await.unwrap();
    let key = EffectKey {
        record: key("reconcile"),
        id: "external".into(),
    };
    let EffectClaimOutcome::Claimed(first) =
        store.claim_effect(&key, 1, "one", 10_000).await.unwrap()
    else {
        panic!()
    };
    store
        .acknowledge_effect(
            &first.lease.unwrap(),
            EffectOutcome::Uncertain { evidence: data(20) },
        )
        .await
        .unwrap();
    let recon = store
        .claim_reconciliation(&key, "reconciler", 10_000)
        .await
        .unwrap();
    assert!(matches!(
        store
            .claim_reconciliation(&key, "second-reconciler", 10_000)
            .await,
        Err(StoreError::Claimed)
    ));
    let lease = recon.lease.unwrap();
    assert!(matches!(
        store
            .acknowledge_effect(&lease, EffectOutcome::Succeeded { receipt: data(20) })
            .await,
        Err(StoreError::Fenced)
    ));
    store
        .reconcile_effect(
            &lease,
            Reconciliation::NotApplied {
                evidence: data(404),
            },
        )
        .await
        .unwrap();
    let EffectClaimOutcome::Claimed(retry) =
        store.claim_effect(&key, 1, "retry", 10_000).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(retry.attempt, 2);
    let events = store.read_events(&key.record, 0, 100).await.unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.kind == "toolkit.store.effect.reconciled"
                && event.data.body["evidence"]["NotApplied"]["evidence"]["body"]["value"] == 404)
    );
}

#[test]
fn rejects_volatile_paths_future_schema_and_unsafe_debug_output() {
    assert!(SqliteStore::open(":memory:").is_err());
    assert!(SqliteStore::open("").is_err());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("future.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    assert!(matches!(
        SqliteStore::open(path),
        Err(StoreError::UnsupportedFormat(99))
    ));
    let body = RecordData {
        schema: "example".into(),
        version: 1,
        body: json!({"private": "do-not-log"}),
    };
    assert!(!format!("{body:?}").contains("do-not-log"));
}

#[tokio::test]
async fn cross_process_claim_child() {
    let Ok(path) = std::env::var("TOOLKIT_STORE_CLAIM_CHILD_DB") else {
        return;
    };
    let worker = std::env::var("TOOLKIT_STORE_CLAIM_CHILD_WORKER").unwrap();
    let store = SqliteStore::open(Path::new(&path)).unwrap();
    match store
        .claim_record(&key("cross-process"), 1, &worker, 60_000)
        .await
    {
        Ok(_) => std::process::exit(0),
        Err(StoreError::Claimed) => std::process::exit(3),
        result => panic!("unexpected claim result: {result:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_process_claims_have_one_winner_on_real_database() {
    let (directory, store) = fixture();
    store.admit(admission("cross-process")).await.unwrap();
    let executable = std::env::current_exe().unwrap();
    let mut children = vec![];
    for id in 0..8 {
        children.push(
            Command::new(&executable)
                .args(["--exact", "cross_process_claim_child", "--nocapture"])
                .env(
                    "TOOLKIT_STORE_CLAIM_CHILD_DB",
                    directory.path().join("store.sqlite"),
                )
                .env("TOOLKIT_STORE_CLAIM_CHILD_WORKER", format!("process-{id}"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let mut winners = 0;
    let mut rejected = 0;
    for child in children {
        let result = child.wait_with_output().unwrap();
        match result.status.code() {
            Some(0) => winners += 1,
            Some(3) => rejected += 1,
            other => panic!(
                "child {other:?}: {}",
                String::from_utf8_lossy(&result.stderr)
            ),
        }
    }
    assert_eq!((winners, rejected), (1, 7));
}

#[tokio::test]
async fn dispatch_admission_rejects_an_obsolete_owner_revision() {
    let (_directory, store) = fixture();
    let mut request = admission("cancelled-owner");
    request
        .effects
        .push(effect("never-send", ReplaySafety::ReconcileBeforeRetry));
    let record = store.admit(request).await.unwrap().record().clone();
    let current = store
        .compare_and_swap(transition(&record, 2))
        .await
        .unwrap();
    let key = EffectKey {
        record: record.key,
        id: "never-send".into(),
    };
    assert!(matches!(
        store.claim_effect(&key, 1, "stale-worker", 1000).await,
        Err(StoreError::Conflict)
    ));
    assert_eq!(
        store.get_effect(&key).await.unwrap().unwrap().status,
        EffectStatus::Pending
    );
    // Only the domain owner can decide whether the current revision allows it.
    assert_eq!(current.revision, 2);
}

#[tokio::test]
async fn concurrent_admission_creates_one_operation_and_one_effect() {
    let (directory, store) = fixture();
    let mut request = admission("concurrent-admission");
    request
        .effects
        .push(effect("once", ReplaySafety::Idempotent));
    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = vec![];
    for _ in 0..8 {
        let contender = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
        let barrier = barrier.clone();
        let request = request.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            contender.admit(request).await.unwrap()
        }));
    }
    let mut created = 0;
    for task in tasks {
        if matches!(task.await.unwrap(), AdmissionOutcome::Created(_)) {
            created += 1;
        }
    }
    assert_eq!(created, 1);
    assert_eq!(
        store.read_events(&request.key, 0, 100).await.unwrap().len(),
        1
    );
    assert_eq!(
        store
            .list_effects(&request.key, None, 100)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn data_digest_is_canonical_and_includes_schema_version() {
    let first = RecordData {
        schema: "a".into(),
        version: 1,
        body: serde_json::from_str(r#"{"b":2,"a":{"z":0,"x":1}}"#).unwrap(),
    };
    let mut second = RecordData {
        schema: "a".into(),
        version: 1,
        body: serde_json::from_str(r#"{"a":{"x":1,"z":0},"b":2}"#).unwrap(),
    };
    assert_eq!(first.digest().unwrap(), second.digest().unwrap());
    second.version = 2;
    assert_ne!(first.digest().unwrap(), second.digest().unwrap());
}

#[tokio::test]
async fn lost_admission_ack_child() {
    let Ok(path) = std::env::var("TOOLKIT_STORE_LOST_ACK_CHILD_DB") else {
        return;
    };
    let store = SqliteStore::open(Path::new(&path)).unwrap();
    let mut request = admission("lost-ack");
    request
        .effects
        .push(effect("committed-once", ReplaySafety::ReconcileBeforeRetry));
    store.admit(request).await.unwrap();
    // Exit without destructors or a result handoff. The parent only sees process
    // loss, then must reconcile admission against the actual persistent store.
    std::process::exit(73);
}

#[tokio::test]
async fn process_loss_after_commit_recovers_without_duplicate_effect() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("lost-ack.sqlite");
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lost_admission_ack_child", "--nocapture"])
        .env("TOOLKIT_STORE_LOST_ACK_CHILD_DB", &path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(
        child.status.code(),
        Some(73),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    let store = SqliteStore::open(path).unwrap();
    let mut request = admission("lost-ack");
    request
        .effects
        .push(effect("committed-once", ReplaySafety::ReconcileBeforeRetry));
    assert!(matches!(
        store.admit(request.clone()).await.unwrap(),
        AdmissionOutcome::Existing(_)
    ));
    assert_eq!(
        store.read_events(&request.key, 0, 100).await.unwrap().len(),
        1
    );
    let effects = store.list_effects(&request.key, None, 100).await.unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].attempt, 0);
}

#[tokio::test]
async fn lost_renewal_result_child() {
    let Ok(directory) = std::env::var("TOOLKIT_STORE_RENEWAL_CHILD_DIR") else {
        return;
    };
    let directory = Path::new(&directory);
    let store = SqliteStore::open(directory.join("store.sqlite")).unwrap();
    let prior: RecordLease =
        serde_json::from_slice(&std::fs::read(directory.join("prior.json")).unwrap()).unwrap();
    store.renew_record_claim(&prior, 60_000).await.unwrap();
    // The initiating process receives no replacement token. Skip destructors
    // and force it to reconcile the committed renewal after reopening.
    std::process::exit(73);
}

#[tokio::test]
async fn lost_renewal_result_is_recoverable_without_reclaiming_a_successor() {
    let (directory, store) = fixture();
    let record = store
        .admit(admission("renewal-recovery"))
        .await
        .unwrap()
        .record()
        .clone();
    let prior = store
        .claim_record(&record.key, 1, "worker", 10_000)
        .await
        .unwrap();
    std::fs::write(
        directory.path().join("prior.json"),
        serde_json::to_vec(&prior).unwrap(),
    )
    .unwrap();
    run_recovery_child(
        "lost_renewal_result_child",
        "TOOLKIT_STORE_RENEWAL_CHILD_DIR",
        directory.path(),
        None,
    );
    drop(store);
    let reopened = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    let recovered = reopened.recover_record_claim(&prior).await.unwrap();
    assert_eq!(recovered.fence, prior.fence);
    assert_eq!(recovered.worker, prior.worker);
    assert!(recovered.expires_at_ms > prior.expires_at_ms);
    assert!(matches!(
        reopened
            .claim_record(&record.key, 1, "contender", 1000)
            .await,
        Err(StoreError::Claimed)
    ));
    let mut change = transition(&record, 2);
    change.claim = Some(prior.clone());
    assert!(matches!(
        reopened.compare_and_swap(change).await,
        Err(StoreError::Fenced)
    ));
    assert!(matches!(
        reopened.renew_record_claim(&prior, 1000).await,
        Err(StoreError::Fenced)
    ));
    assert!(matches!(
        reopened.release_record_claim(&prior).await,
        Err(StoreError::Fenced)
    ));
    let mut change = transition(&record, 2);
    change.claim = Some(recovered.clone());
    let updated = reopened.compare_and_swap(change).await.unwrap();
    reopened.release_record_claim(&recovered).await.unwrap();
    assert!(matches!(
        reopened.recover_record_claim(&prior).await,
        Err(StoreError::Fenced)
    ));
    // Even reuse of the same worker identity starts a new executor generation.
    let successor = reopened
        .claim_record(&record.key, updated.revision, "worker", 10_000)
        .await
        .unwrap();
    assert!(successor.fence > prior.fence);
    assert!(matches!(
        reopened.recover_record_claim(&prior).await,
        Err(StoreError::Fenced)
    ));
    assert_eq!(
        reopened.recover_record_claim(&successor).await.unwrap(),
        successor
    );
}

#[tokio::test]
async fn claim_recovery_requires_the_original_live_store_worker_and_fence() {
    let (_directory, store) = fixture();
    let record = store
        .admit(admission("recovery-boundary"))
        .await
        .unwrap()
        .record()
        .clone();
    let prior = store
        .claim_record(&record.key, 1, "worker", 500)
        .await
        .unwrap();
    let renewed = store.renew_record_claim(&prior, 10_000).await.unwrap();
    wait_past(&store, prior.expires_at_ms).await;
    assert_eq!(store.recover_record_claim(&prior).await.unwrap(), renewed);
    for changed in ["store", "realm", "owner", "record", "worker", "fence"] {
        let mut forged = prior.clone();
        match changed {
            "store" => forged.store_id = "another-store".into(),
            "realm" => forged.key.realm = "another-realm".into(),
            "owner" => forged.key.owner = "another-owner".into(),
            "record" => forged.key.id = "another-record".into(),
            "worker" => forged.worker = "another-worker".into(),
            "fence" => forged.fence += 1,
            _ => unreachable!(),
        }
        assert!(matches!(
            store.recover_record_claim(&forged).await,
            Err(StoreError::Fenced)
        ));
    }
    store.release_record_claim(&renewed).await.unwrap();
    let expiring = store
        .claim_record(&record.key, 1, "worker", 20)
        .await
        .unwrap();
    wait_past(&store, expiring.expires_at_ms).await;
    assert!(matches!(
        store.recover_record_claim(&expiring).await,
        Err(StoreError::Fenced)
    ));
}

fn run_recovery_child(test: &str, directory_variable: &str, directory: &Path, stage: Option<&str>) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture"])
        .env(directory_variable, directory)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    if let Some(stage) = stage {
        command.env("TOOLKIT_STORE_EFFECT_CHILD_STAGE", stage);
    }
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(73),
        "child stage {stage:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn crash_request() -> AdmissionRequest {
    let mut request = admission("crash-boundary");
    request.effects.push(effect(
        "non-idempotent-write",
        ReplaySafety::ReconcileBeforeRetry,
    ));
    request
}

fn crash_key() -> EffectKey {
    EffectKey {
        record: key("crash-boundary"),
        id: "non-idempotent-write".into(),
    }
}

/// A separate persistent database simulates an external service. Every call
/// appends another row, including duplicate idempotency keys, so these tests
/// cannot hide accidental repeats behind an idempotent simulator.
fn simulator(directory: &Path) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(directory.join("external.sqlite")).unwrap();
    connection
        .pragma_update(None, "synchronous", "FULL")
        .unwrap();
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS external_writes (
             id INTEGER PRIMARY KEY, request_digest TEXT NOT NULL,
             idempotency_key TEXT NOT NULL, request TEXT NOT NULL)",
        )
        .unwrap();
    connection
}

fn simulator_apply(directory: &Path, effect: &StoredEffect) -> RecordData {
    let connection = simulator(directory);
    connection
        .execute(
            "INSERT INTO external_writes(request_digest,idempotency_key,request) VALUES(?1,?2,?3)",
            rusqlite::params![
                effect.request_digest,
                effect.intent.idempotency_key,
                serde_json::to_string(&effect.intent.request).unwrap(),
            ],
        )
        .unwrap();
    simulated_receipt(effect, connection.last_insert_rowid())
}

fn simulated_receipt(effect: &StoredEffect, row: i64) -> RecordData {
    RecordData {
        schema: "test.external-receipt".into(),
        version: 1,
        body: json!({
            "external_row": row,
            "request_digest": effect.request_digest,
            "idempotency_key": effect.intent.idempotency_key,
        }),
    }
}

fn simulator_lookup(directory: &Path, effect: &StoredEffect) -> Option<RecordData> {
    let connection = simulator(directory);
    let mut statement = connection
        .prepare(
            "SELECT id,request_digest,idempotency_key,request FROM external_writes ORDER BY id",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(rows.len() <= 1, "non-idempotent effect was repeated");
    rows.first().map(|(row, digest, idempotency, request)| {
        assert_eq!(digest, &effect.request_digest);
        assert_eq!(idempotency, &effect.intent.idempotency_key);
        assert_eq!(
            serde_json::from_str::<RecordData>(request).unwrap(),
            effect.intent.request
        );
        simulated_receipt(effect, *row)
    })
}

#[tokio::test]
async fn effect_recovery_boundary_child() {
    let Ok(directory) = std::env::var("TOOLKIT_STORE_EFFECT_CHILD_DIR") else {
        return;
    };
    let directory = Path::new(&directory);
    let stage = std::env::var("TOOLKIT_STORE_EFFECT_CHILD_STAGE").unwrap();
    let store = SqliteStore::open(directory.join("store.sqlite")).unwrap();
    store.admit(crash_request()).await.unwrap();
    let lease_ms = if matches!(stage.as_str(), "dispatch-admitted" | "external-applied") {
        1000
    } else {
        60_000
    };
    let EffectClaimOutcome::Claimed(effect) = store
        .claim_effect(&crash_key(), 1, "lost-process", lease_ms)
        .await
        .unwrap()
    else {
        panic!("initial dispatch must be claimed")
    };
    if stage == "dispatch-admitted" {
        std::process::exit(73);
    }
    let receipt = simulator_apply(directory, &effect);
    if stage == "external-applied" {
        std::process::exit(73);
    }
    if stage == "acknowledgment-committed" {
        store
            .acknowledge_effect(
                effect.lease.as_ref().unwrap(),
                EffectOutcome::Succeeded { receipt },
            )
            .await
            .unwrap();
        std::process::exit(73);
    }
    assert_eq!(stage, "reconciliation-committed");
    store
        .acknowledge_effect(
            effect.lease.as_ref().unwrap(),
            EffectOutcome::Uncertain {
                evidence: data(503),
            },
        )
        .await
        .unwrap();
    let reconciliation = store
        .claim_reconciliation(&crash_key(), "owner", 10_000)
        .await
        .unwrap();
    // The service owner queries the independent external record before
    // resolving the ambiguity. Its typed receipt is retained with the event.
    let receipt = simulator_lookup(directory, &effect).unwrap();
    store
        .reconcile_effect(
            reconciliation.lease.as_ref().unwrap(),
            Reconciliation::Applied { receipt },
        )
        .await
        .unwrap();
    std::process::exit(73);
}

async fn recovered_crash_effect(directory: &Path, stage: &str) -> (SqliteStore, StoredEffect) {
    run_recovery_child(
        "effect_recovery_boundary_child",
        "TOOLKIT_STORE_EFFECT_CHILD_DIR",
        directory,
        Some(stage),
    );
    let store = SqliteStore::open(directory.join("store.sqlite")).unwrap();
    assert!(matches!(
        store.admit(crash_request()).await.unwrap(),
        AdmissionOutcome::Existing(_)
    ));
    let effect = store.get_effect(&crash_key()).await.unwrap().unwrap();
    assert_eq!(effect.intent, crash_request().effects[0]);
    assert_eq!(effect.attempt, 1);
    assert_eq!(
        store
            .list_effects(&effect.key.record, None, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    (store, effect)
}

async fn require_reconciliation(store: &SqliteStore, effect: &StoredEffect) -> StoredEffect {
    wait_past(store, effect.lease.as_ref().unwrap().expires_at_ms).await;
    let EffectClaimOutcome::NeedsReconciliation(uncertain) = store
        .claim_effect(&effect.key, 1, "replacement", 10_000)
        .await
        .unwrap()
    else {
        panic!("lost non-idempotent dispatch must not be repeated")
    };
    assert_eq!(uncertain.intent, effect.intent);
    assert_eq!(uncertain.request_digest, effect.request_digest);
    assert_eq!(uncertain.attempt, 1);
    assert_eq!(uncertain.status, EffectStatus::Uncertain);
    let events = store.read_events(&effect.key.record, 0, 100).await.unwrap();
    assert_eq!(events.last().unwrap().data.body["status"], "Uncertain");
    store
        .claim_reconciliation(&effect.key, "reconciler", 10_000)
        .await
        .unwrap()
}

async fn assert_terminal_evidence(
    store: &SqliteStore,
    effect: &StoredEffect,
    event_kind: &str,
    receipt: &RecordData,
) {
    assert_eq!(effect.status, EffectStatus::Succeeded);
    assert_eq!(
        effect.outcome,
        Some(EffectOutcome::Succeeded {
            receipt: receipt.clone()
        })
    );
    assert!(effect.lease.is_none());
    assert!(matches!(
        store
            .claim_effect(&effect.key, 1, "late-dispatch", 1000)
            .await,
        Err(StoreError::EffectState)
    ));
    assert!(matches!(
        store
            .claim_reconciliation(&effect.key, "late-owner", 1000)
            .await,
        Err(StoreError::EffectState)
    ));
    let events = store.read_events(&effect.key.record, 0, 100).await.unwrap();
    let terminal = events.last().unwrap();
    assert_eq!(terminal.kind, event_kind);
    assert_eq!(terminal.data.body["status"], "Succeeded");
    assert_eq!(terminal.data.body["request_digest"], effect.request_digest);
    let branch = if event_kind.ends_with("reconciled") {
        "Applied"
    } else {
        "Succeeded"
    };
    assert_eq!(
        terminal.data.body["evidence"][branch]["receipt"],
        serde_json::to_value(receipt).unwrap()
    );
    assert_eq!(
        store.get_effect(&effect.key).await.unwrap().as_ref(),
        Some(effect)
    );
}

#[tokio::test]
async fn process_loss_after_dispatch_admission_requires_proven_absence_before_retry() {
    let directory = tempfile::tempdir().unwrap();
    let (store, effect) = recovered_crash_effect(directory.path(), "dispatch-admitted").await;
    assert!(simulator_lookup(directory.path(), &effect).is_none());
    let reconciliation = require_reconciliation(&store, &effect).await;
    let absence = RecordData {
        schema: "test.external-query".into(),
        version: 1,
        body: json!({ "request_digest": effect.request_digest, "found": false }),
    };
    store
        .reconcile_effect(
            reconciliation.lease.as_ref().unwrap(),
            Reconciliation::NotApplied {
                evidence: absence.clone(),
            },
        )
        .await
        .unwrap();
    let EffectClaimOutcome::Claimed(retry) = store
        .claim_effect(&effect.key, 1, "authorized-retry", 10_000)
        .await
        .unwrap()
    else {
        panic!("reconciled absence permits the authorized retry")
    };
    assert_eq!(retry.attempt, 2);
    assert_eq!(retry.intent, effect.intent);
    assert_eq!(retry.request_digest, effect.request_digest);
    let receipt = simulator_apply(directory.path(), &retry);
    let terminal = store
        .acknowledge_effect(
            retry.lease.as_ref().unwrap(),
            EffectOutcome::Succeeded {
                receipt: receipt.clone(),
            },
        )
        .await
        .unwrap();
    assert_terminal_evidence(
        &store,
        &terminal,
        "toolkit.store.effect.acknowledged",
        &receipt,
    )
    .await;
    assert_eq!(simulator_lookup(directory.path(), &terminal), Some(receipt));
    let events = store.read_events(&effect.key.record, 0, 100).await.unwrap();
    assert!(events.iter().any(|event| {
        event.kind == "toolkit.store.effect.reconciled"
            && event.data.body["evidence"]["NotApplied"]["evidence"]
                == serde_json::to_value(&absence).unwrap()
    }));
}

#[tokio::test]
async fn process_loss_after_external_effect_reconciles_without_repeating_it() {
    let directory = tempfile::tempdir().unwrap();
    let (store, effect) = recovered_crash_effect(directory.path(), "external-applied").await;
    let receipt = simulator_lookup(directory.path(), &effect).unwrap();
    let reconciliation = require_reconciliation(&store, &effect).await;
    let terminal = store
        .reconcile_effect(
            reconciliation.lease.as_ref().unwrap(),
            Reconciliation::Applied {
                receipt: receipt.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(terminal.attempt, 1);
    assert_terminal_evidence(
        &store,
        &terminal,
        "toolkit.store.effect.reconciled",
        &receipt,
    )
    .await;
    assert_eq!(simulator_lookup(directory.path(), &terminal), Some(receipt));
}

#[tokio::test]
async fn process_loss_after_acknowledgment_commit_preserves_terminal_fact_and_event() {
    let directory = tempfile::tempdir().unwrap();
    let (store, effect) =
        recovered_crash_effect(directory.path(), "acknowledgment-committed").await;
    let receipt = simulator_lookup(directory.path(), &effect).unwrap();
    assert_terminal_evidence(
        &store,
        &effect,
        "toolkit.store.effect.acknowledged",
        &receipt,
    )
    .await;
    assert_eq!(simulator_lookup(directory.path(), &effect), Some(receipt));
}

#[tokio::test]
async fn process_loss_after_reconciliation_commit_preserves_receipt_and_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let (store, effect) =
        recovered_crash_effect(directory.path(), "reconciliation-committed").await;
    let receipt = simulator_lookup(directory.path(), &effect).unwrap();
    assert_terminal_evidence(&store, &effect, "toolkit.store.effect.reconciled", &receipt).await;
    assert_eq!(simulator_lookup(directory.path(), &effect), Some(receipt));
}

#[tokio::test]
async fn pending_reconciliation_is_durable_releases_only_its_claim_and_never_authorizes_dispatch() {
    let (directory, store) = fixture();
    let mut request = admission("pending-reconciliation");
    request
        .effects
        .push(effect("external", ReplaySafety::ReconcileBeforeRetry));
    store.admit(request).await.unwrap();
    let key = EffectKey {
        record: key("pending-reconciliation"),
        id: "external".into(),
    };
    let EffectClaimOutcome::Claimed(first) =
        store.claim_effect(&key, 1, "one", 10_000).await.unwrap()
    else {
        panic!()
    };
    store
        .acknowledge_effect(
            first.lease.as_ref().unwrap(),
            EffectOutcome::Uncertain { evidence: data(20) },
        )
        .await
        .unwrap();
    let reconciliation = store
        .claim_reconciliation(&key, "observer", 10_000)
        .await
        .unwrap();
    let lease = reconciliation.lease.unwrap();
    let pending = store
        .reconcile_effect(&lease, Reconciliation::Pending { evidence: data(21) })
        .await
        .unwrap();
    assert_eq!(pending.status, EffectStatus::Uncertain);
    assert_eq!(pending.attempt, 1);
    assert_eq!(pending.intent, first.intent);
    assert!(pending.lease.is_none());
    assert_eq!(
        pending.outcome,
        Some(EffectOutcome::Uncertain { evidence: data(21) })
    );
    assert!(matches!(
        store
            .reconcile_effect(&lease, Reconciliation::NotApplied { evidence: data(22) })
            .await,
        Err(StoreError::Fenced)
    ));
    drop(store);
    let reopened = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    assert_eq!(reopened.get_effect(&key).await.unwrap().unwrap(), pending);
    assert!(matches!(
        reopened
            .claim_effect(&key, 1, "retry", 10_000)
            .await
            .unwrap(),
        EffectClaimOutcome::NeedsReconciliation(_)
    ));
    let next = reopened
        .claim_reconciliation(&key, "next-observer", 10_000)
        .await
        .unwrap();
    assert_eq!(next.attempt, 1);
    assert!(next.lease.unwrap().fence > lease.fence);
    let events = reopened.read_events(&key.record, 0, 100).await.unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.kind == "toolkit.store.effect.reconciled"
                && event.data.body["evidence"]["Pending"]["evidence"]["body"]["value"] == 21)
    );
}

#[tokio::test]
async fn retry_safe_and_not_applied_keep_distinct_evidence_without_resetting_attempts() {
    for (name, resolution) in [
        (
            "RetrySafe",
            Reconciliation::RetrySafe { evidence: data(81) },
        ),
        (
            "NotApplied",
            Reconciliation::NotApplied { evidence: data(82) },
        ),
    ] {
        let (directory, store) = fixture();
        let mut request = admission(name);
        request
            .effects
            .push(effect("external", ReplaySafety::ReconcileBeforeRetry));
        store.admit(request).await.unwrap();
        let key = EffectKey {
            record: key(name),
            id: "external".into(),
        };
        let EffectClaimOutcome::Claimed(first) = store
            .claim_effect(&key, 1, "dispatch", 10_000)
            .await
            .unwrap()
        else {
            panic!()
        };
        let dispatch = first.lease.as_ref().unwrap();
        assert!(matches!(
            store.reconcile_effect(dispatch, resolution.clone()).await,
            Err(StoreError::Fenced)
        ));
        store
            .acknowledge_effect(dispatch, EffectOutcome::Uncertain { evidence: data(80) })
            .await
            .unwrap();
        let observed = store
            .claim_reconciliation(&key, "observer", 10_000)
            .await
            .unwrap();
        let lease = observed.lease.as_ref().unwrap();
        let pending = store
            .reconcile_effect(lease, resolution.clone())
            .await
            .unwrap();
        assert_eq!(pending.status, EffectStatus::Pending);
        assert_eq!(pending.attempt, 1);
        assert!(pending.outcome.is_none() && pending.lease.is_none());
        assert_eq!(pending.intent, first.intent);
        assert_eq!(pending.request_digest, first.request_digest);
        assert!(matches!(
            store.reconcile_effect(lease, resolution.clone()).await,
            Err(StoreError::Fenced)
        ));
        drop(store);
        let reopened = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
        assert_eq!(reopened.get_effect(&key).await.unwrap(), Some(pending));
        let events = reopened.read_events(&key.record, 0, 100).await.unwrap();
        let saved = events
            .iter()
            .find(|event| event.kind == "toolkit.store.effect.reconciled")
            .unwrap();
        assert_eq!(
            saved.data.body["evidence"],
            serde_json::to_value(&resolution).unwrap()
        );
        assert!(saved.data.body["evidence"].get(name).is_some());
        let EffectClaimOutcome::Claimed(retry) = reopened
            .claim_effect(&key, 1, "retry", 10_000)
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(retry.attempt, 2);
        assert_eq!(retry.intent, first.intent);
        assert_eq!(retry.request_digest, first.request_digest);
    }
    assert_ne!(
        serde_json::to_value(Reconciliation::RetrySafe { evidence: data(1) }).unwrap(),
        serde_json::to_value(Reconciliation::NotApplied { evidence: data(1) }).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attempted_pending_effect_can_be_observed_once_without_another_dispatch_claim() {
    let (directory, store) = fixture();
    let mut request = admission("recover-retry-safe");
    request
        .effects
        .push(effect("external", ReplaySafety::ReconcileBeforeRetry));
    store.admit(request).await.unwrap();
    let key = EffectKey {
        record: key("recover-retry-safe"),
        id: "external".into(),
    };
    assert!(matches!(
        store.claim_reconciliation(&key, "fresh", 10_000).await,
        Err(StoreError::EffectState)
    ));
    let EffectClaimOutcome::Claimed(first) = store
        .claim_effect(&key, 1, "dispatch", 10_000)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(matches!(
        store
            .claim_reconciliation(&key, "live-dispatch", 10_000)
            .await,
        Err(StoreError::Claimed)
    ));
    store
        .acknowledge_effect(
            first.lease.as_ref().unwrap(),
            EffectOutcome::Uncertain { evidence: data(91) },
        )
        .await
        .unwrap();
    let observed = store
        .claim_reconciliation(&key, "initial-observer", 10_000)
        .await
        .unwrap();
    store
        .reconcile_effect(
            observed.lease.as_ref().unwrap(),
            Reconciliation::RetrySafe { evidence: data(92) },
        )
        .await
        .unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for n in 0..16 {
        let reopened = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
        let key = key.clone();
        tasks.spawn(async move {
            reopened
                .claim_reconciliation(&key, &format!("observer-{n}"), 10_000)
                .await
        });
    }
    let mut winner = None;
    let mut refused = 0;
    while let Some(result) = tasks.join_next().await {
        match result.unwrap() {
            Ok(effect) => {
                assert!(winner.replace(effect).is_none());
            }
            Err(StoreError::Claimed) => refused += 1,
            other => panic!("unexpected claim result: {other:?}"),
        }
    }
    assert_eq!(refused, 15);
    let winner = winner.unwrap();
    assert_eq!(winner.attempt, 1);
    assert_eq!(winner.status, EffectStatus::Uncertain);
    assert_eq!(winner.intent, first.intent);
    assert!(matches!(
        store
            .claim_effect(&key, 1, "competing-dispatch", 10_000)
            .await,
        Err(StoreError::Claimed)
    ));
    let lease = winner.lease.as_ref().unwrap();
    store
        .reconcile_effect(lease, Reconciliation::Applied { receipt: data(93) })
        .await
        .unwrap();
    assert!(matches!(
        store
            .claim_reconciliation(&key, "already-applied", 10_000)
            .await,
        Err(StoreError::EffectState)
    ));
    assert!(matches!(
        store
            .reconcile_effect(lease, Reconciliation::RetrySafe { evidence: data(94) })
            .await,
        Err(StoreError::Fenced)
    ));
    assert_eq!(store.get_effect(&key).await.unwrap().unwrap().attempt, 1);
    let events = store.read_events(&key.record, 0, 100).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "toolkit.store.effect.claimed")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "toolkit.store.effect.reconciliation-claimed")
            .count(),
        2
    );
}

#[tokio::test]
async fn attempted_pending_reconciliation_preserves_expiry_terminal_and_revision_fences() {
    let (_directory, store) = fixture();
    let mut request = admission("retry-fences");
    request
        .effects
        .push(effect("external", ReplaySafety::ReconcileBeforeRetry));
    let record = store.admit(request).await.unwrap().record().clone();
    let key = EffectKey {
        record: record.key.clone(),
        id: "external".into(),
    };
    let EffectClaimOutcome::Claimed(first) = store
        .claim_effect(&key, 1, "dispatch", 10_000)
        .await
        .unwrap()
    else {
        panic!()
    };
    store
        .acknowledge_effect(
            first.lease.as_ref().unwrap(),
            EffectOutcome::Uncertain { evidence: data(71) },
        )
        .await
        .unwrap();
    let initial = store
        .claim_reconciliation(&key, "initial-observer", 10_000)
        .await
        .unwrap();
    store
        .reconcile_effect(
            initial.lease.as_ref().unwrap(),
            Reconciliation::RetrySafe { evidence: data(72) },
        )
        .await
        .unwrap();
    let current = store
        .compare_and_swap(transition(&record, 2))
        .await
        .unwrap();
    assert!(matches!(
        store
            .claim_effect(&key, record.revision, "stale-dispatch", 10_000)
            .await,
        Err(StoreError::Conflict)
    ));
    let observed = store
        .claim_reconciliation(&key, "recovery", 1)
        .await
        .unwrap();
    let expired = observed.lease.unwrap();
    wait_past(&store, expired.expires_at_ms).await;
    assert!(matches!(
        store
            .reconcile_effect(&expired, Reconciliation::Applied { receipt: data(73) })
            .await,
        Err(StoreError::Fenced)
    ));
    let recovered = store
        .claim_reconciliation(&key, "current-recovery", 10_000)
        .await
        .unwrap();
    assert_eq!(recovered.attempt, 1);
    assert!(recovered.lease.as_ref().unwrap().fence > expired.fence);
    store
        .reconcile_effect(
            recovered.lease.as_ref().unwrap(),
            Reconciliation::Failed { evidence: data(74) },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .claim_reconciliation(&key, "failed-recovery", 10_000)
            .await,
        Err(StoreError::EffectState)
    ));
    assert!(matches!(
        store
            .claim_effect(&key, current.revision, "failed-dispatch", 10_000)
            .await,
        Err(StoreError::EffectState)
    ));
    assert_eq!(store.get_effect(&key).await.unwrap().unwrap().attempt, 1);
}
