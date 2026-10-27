use serde_json::json;
use std::process::Command;
use std::sync::Arc;
use toolkit_store::*;

fn data(value: u64) -> RecordData {
    RecordData {
        schema: "test.conditional".into(),
        version: 1,
        body: json!({"value": value}),
    }
}

fn key(id: &str) -> StoreKey {
    StoreKey {
        realm: "realm".into(),
        owner: "owner".into(),
        id: id.into(),
    }
}

fn admission(id: &str) -> AdmissionRequest {
    AdmissionRequest {
        key: key(id),
        admission_key: format!("admit-{id}"),
        data: data(1),
        events: vec![EventWrite {
            kind: "published".into(),
            data: data(1),
        }],
        effects: vec![EffectIntent {
            id: "work".into(),
            kind: "test.work".into(),
            request: data(1),
            idempotency_key: format!("work-{id}"),
            replay_safety: ReplaySafety::ReconcileBeforeRetry,
        }],
    }
}

fn condition(record: &StoredRecord) -> RecordCondition {
    RecordCondition {
        key: record.key.clone(),
        revision: record.revision,
        content_digest: record.content_digest.clone(),
        claim: None,
        dispatch_claim: None,
    }
}

fn transition(record: &StoredRecord) -> Transition {
    Transition {
        key: record.key.clone(),
        expected_revision: record.revision,
        expected_digest: Some(record.content_digest.clone()),
        claim: None,
        data: data(2),
        events: vec![EventWrite {
            kind: "revoked".into(),
            data: data(2),
        }],
        effects: vec![],
    }
}

async fn owner(store: &SqliteStore, id: &str) -> StoredRecord {
    store.admit(admission(id)).await.unwrap().record().clone()
}

async fn request(store: &SqliteStore, record: &StoredRecord, id: &str) -> ConditionalAdmission {
    ConditionalAdmission {
        admission: admission(id),
        conditions: vec![condition(record)],
        before_ms: store.now_ms().await.unwrap() + 60_000,
    }
}

async fn assert_absent(store: &SqliteStore, key: &StoreKey) {
    assert!(store.get(key).await.unwrap().is_none());
    assert!(store.read_events(key, 0, 100).await.unwrap().is_empty());
    assert!(store.list_effects(key, None, 100).await.unwrap().is_empty());
}

async fn wait_past(store: &SqliteStore, time: i64) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while store.now_ms().await.unwrap() <= time {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn conditional_reopen_replays_history_after_revision_and_claim_change() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let policy = owner(&store, "policy").await;
    let cancellation = owner(&store, "cancellation").await;
    let lease = store
        .claim_record(&policy.key, policy.revision, "worker", 60_000)
        .await
        .unwrap();
    let mut prepared = request(&store, &policy, "child").await;
    prepared.conditions[0].claim = Some(lease.clone());
    prepared.conditions.push(condition(&cancellation));
    let result = store.admit_conditionally(prepared.clone()).await.unwrap();
    assert!(matches!(result, AdmissionOutcome::Created(_)));
    store.compare_and_swap(transition(&policy)).await.unwrap();
    store
        .compare_and_swap(transition(&cancellation))
        .await
        .unwrap();
    store.release_record_claim(&lease).await.unwrap();
    // Exact replay observes the old commit even with an expired continuation.
    prepared.before_ms = 1;
    drop(store);
    let reopened = SqliteStore::open(path).unwrap();
    let replay = reopened
        .admit_conditionally(prepared.clone())
        .await
        .unwrap();
    assert!(matches!(replay, AdmissionOutcome::Existing(_)));
    assert_eq!(replay.record(), result.record());
    assert_eq!(
        reopened
            .read_events(&key("child"), 0, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    let effects = reopened
        .list_effects(&key("child"), None, 100)
        .await
        .unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].attempt, 0);
    assert_eq!(effects[0].status, EffectStatus::Pending);
    prepared.admission.data = data(7);
    assert!(matches!(
        reopened.admit_conditionally(prepared).await,
        Err(StoreError::AdmissionConflict)
    ));
}

#[tokio::test]
async fn malformed_and_stale_conditions_leave_no_record_event_or_effect() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    let policy = owner(&store, "policy").await;
    let valid = request(&store, &policy, "child").await;
    for mutation in 0..10 {
        let mut candidate = valid.clone();
        match mutation {
            0 => candidate.conditions[0].revision += 1,
            1 => candidate.conditions[0].content_digest = "a".repeat(64),
            2 => candidate.conditions[0].key.id = "missing".into(),
            3 => candidate.conditions.push(candidate.conditions[0].clone()),
            4 => candidate.conditions[0].key.realm = "another-realm".into(),
            5 => candidate.conditions.clear(),
            6 => candidate.conditions[0].revision = 0,
            7 => candidate.conditions[0].key = candidate.admission.key.clone(),
            8 => candidate.conditions[0].content_digest = "not-a-digest".into(),
            9 => candidate.before_ms = 1,
            _ => unreachable!(),
        }
        let failure = store.admit_conditionally(candidate).await.unwrap_err();
        match mutation {
            0..=2 => assert!(matches!(failure, StoreError::Conflict)),
            9 => assert!(matches!(failure, StoreError::AdmissionExpired)),
            _ => assert!(matches!(failure, StoreError::Invalid(_))),
        }
        assert_absent(&store, &key("child")).await;
    }
    store.compare_and_swap(transition(&policy)).await.unwrap();
    assert!(matches!(
        store.admit_conditionally(valid).await,
        Err(StoreError::Conflict)
    ));
    assert_absent(&store, &key("child")).await;
}

#[tokio::test]
async fn conditional_effect_collision_rolls_back_whole_admission() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    let policy = owner(&store, "policy").await;
    let mut prepared = request(&store, &policy, "child").await;
    prepared.admission.effects[0].idempotency_key = "work-policy".into();
    assert!(matches!(
        store.admit_conditionally(prepared).await,
        Err(StoreError::EffectConflict)
    ));
    assert_absent(&store, &key("child")).await;
    assert!(matches!(
        store
            .admit_conditionally(request(&store, &policy, "child").await)
            .await
            .unwrap(),
        AdmissionOutcome::Created(_)
    ));
}

#[tokio::test]
async fn record_claim_must_be_exact_current_live_and_from_this_database() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    let other = SqliteStore::open(directory.path().join("other.sqlite")).unwrap();
    let policy = owner(&store, "policy").await;
    let foreign = owner(&other, "policy").await;
    let prior = store
        .claim_record(&policy.key, 1, "worker", 60_000)
        .await
        .unwrap();
    let renewed = store.renew_record_claim(&prior, 120_000).await.unwrap();
    let foreign = other
        .claim_record(&foreign.key, 1, "worker", 120_000)
        .await
        .unwrap();
    for claim in [prior, foreign] {
        let mut prepared = request(&store, &policy, "child").await;
        prepared.conditions[0].claim = Some(claim);
        assert!(matches!(
            store.admit_conditionally(prepared).await,
            Err(StoreError::Fenced)
        ));
        assert_absent(&store, &key("child")).await;
    }
    let mut prepared = request(&store, &policy, "child").await;
    prepared.conditions[0].claim = Some(renewed.clone());
    store.admit_conditionally(prepared).await.unwrap();
    store.release_record_claim(&renewed).await.unwrap();
    let mut next = request(&store, &policy, "next").await;
    next.conditions[0].claim = Some(renewed);
    assert!(matches!(
        store.admit_conditionally(next.clone()).await,
        Err(StoreError::Fenced)
    ));
    let expiring = store
        .claim_record(&policy.key, 1, "worker", 10)
        .await
        .unwrap();
    wait_past(&store, expiring.expires_at_ms).await;
    next.conditions[0].claim = Some(expiring);
    assert!(matches!(
        store.admit_conditionally(next).await,
        Err(StoreError::Fenced)
    ));
    assert_absent(&store, &key("next")).await;
}

#[tokio::test]
async fn dispatch_claim_cannot_be_substituted_by_foreign_settled_expired_or_reconciliation_claim() {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    let other = SqliteStore::open(directory.path().join("other.sqlite")).unwrap();
    let policy = owner(&store, "policy").await;
    owner(&other, "policy").await;
    let effect_key = EffectKey {
        record: policy.key.clone(),
        id: "work".into(),
    };
    let EffectClaimOutcome::Claimed(claimed) = store
        .claim_effect(&effect_key, 1, "worker", 60_000)
        .await
        .unwrap()
    else {
        panic!("fresh dispatch")
    };
    let lease = claimed.lease.unwrap();
    let EffectClaimOutcome::Claimed(foreign) = other
        .claim_effect(&effect_key, 1, "worker", 60_000)
        .await
        .unwrap()
    else {
        panic!("fresh foreign dispatch")
    };
    let mut prepared = request(&store, &policy, "child").await;
    prepared.conditions[0].dispatch_claim = foreign.lease;
    assert!(matches!(
        store.admit_conditionally(prepared.clone()).await,
        Err(StoreError::Fenced)
    ));
    assert_absent(&store, &key("child")).await;
    prepared.conditions[0].dispatch_claim = Some(lease.clone());
    store.admit_conditionally(prepared).await.unwrap();
    store
        .acknowledge_effect(&lease, EffectOutcome::Uncertain { evidence: data(9) })
        .await
        .unwrap();
    let mut next = request(&store, &policy, "next").await;
    next.conditions[0].dispatch_claim = Some(lease);
    assert!(matches!(
        store.admit_conditionally(next.clone()).await,
        Err(StoreError::Fenced)
    ));
    let reconcile = store
        .claim_reconciliation(&effect_key, "observer", 60_000)
        .await
        .unwrap();
    next.conditions[0].dispatch_claim = reconcile.lease;
    assert!(matches!(
        store.admit_conditionally(next).await,
        Err(StoreError::Invalid(_))
    ));
    assert_absent(&store, &key("next")).await;
    let expiring = owner(&store, "expiring").await;
    let expiring_key = EffectKey {
        record: expiring.key.clone(),
        id: "work".into(),
    };
    let EffectClaimOutcome::Claimed(expired) = store
        .claim_effect(&expiring_key, 1, "worker", 10)
        .await
        .unwrap()
    else {
        panic!("fresh expiring dispatch")
    };
    let expired = expired.lease.unwrap();
    wait_past(&store, expired.expires_at_ms).await;
    let mut expired_request = request(&store, &expiring, "expired-child").await;
    expired_request.conditions[0].dispatch_claim = Some(expired);
    assert!(matches!(
        store.admit_conditionally(expired_request).await,
        Err(StoreError::Fenced)
    ));
    assert_absent(&store, &key("expired-child")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deadline_is_rechecked_after_waiting_for_another_connection_writer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let policy = owner(&store, "policy").await;
    let mut prepared = request(&store, &policy, "child").await;
    prepared.before_ms = store.now_ms().await.unwrap() + 100;
    let deadline = prepared.before_ms;
    let mut lock_connection = rusqlite::Connection::open(&path).unwrap();
    let lock = lock_connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let writer = store.clone();
    let task = tokio::spawn(async move { writer.admit_conditionally(prepared).await });
    // A separate connection can still read the database clock under WAL.
    while lock
        .query_row(
            "SELECT CAST(unixepoch('subsec') * 1000 AS INTEGER)",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        <= deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    lock.commit().unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(StoreError::AdmissionExpired)
    ));
    assert_absent(&store, &key("child")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_independent_connections_publish_once_then_refuse_stale_owner() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let policy = owner(&store, "policy").await;
    let prepared = request(&store, &policy, "child").await;
    let barrier = Arc::new(tokio::sync::Barrier::new(8));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let writer = SqliteStore::open(&path).unwrap();
        let request = prepared.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            writer.admit_conditionally(request).await.unwrap()
        }));
    }
    let mut created = 0;
    for task in tasks {
        created += usize::from(matches!(task.await.unwrap(), AdmissionOutcome::Created(_)));
    }
    assert_eq!(created, 1);
    assert_eq!(
        store
            .read_events(&key("child"), 0, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .list_effects(&key("child"), None, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    let writer = SqliteStore::open(path).unwrap();
    writer.compare_and_swap(transition(&policy)).await.unwrap();
    assert!(matches!(
        store
            .admit_conditionally(request(&store, &policy, "after-revoke").await)
            .await,
        Err(StoreError::Conflict)
    ));
    assert_absent(&store, &key("after-revoke")).await;
}

#[tokio::test]
async fn conditional_process_child() {
    let Ok(path) = std::env::var("TOOLKIT_CONDITIONAL_CHILD_DB") else {
        return;
    };
    let store = SqliteStore::open(path).unwrap();
    let policy = store.get(&key("policy")).await.unwrap().unwrap();
    match std::env::var("TOOLKIT_CONDITIONAL_CHILD_MODE")
        .unwrap()
        .as_str()
    {
        "publish" => {
            store
                .admit_conditionally(request(&store, &policy, "child").await)
                .await
                .unwrap();
            std::process::exit(73);
        }
        "revoke" => {
            store.compare_and_swap(transition(&policy)).await.unwrap();
        }
        _ => panic!("invalid child mode"),
    }
}

#[tokio::test]
async fn child_process_commit_without_ack_is_recoverable_after_another_process_revokes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("store.sqlite");
    let store = SqliteStore::open(&path).unwrap();
    let policy = owner(&store, "policy").await;
    let prepared = request(&store, &policy, "child").await;
    for (mode, exit) in [("publish", 73), ("revoke", 0)] {
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "conditional_process_child", "--nocapture"])
            .env("TOOLKIT_CONDITIONAL_CHILD_DB", &path)
            .env("TOOLKIT_CONDITIONAL_CHILD_MODE", mode)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(exit),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    drop(store);
    let reopened = SqliteStore::open(path).unwrap();
    assert!(matches!(
        reopened.admit_conditionally(prepared).await.unwrap(),
        AdmissionOutcome::Existing(_)
    ));
    assert_eq!(
        reopened
            .read_events(&key("child"), 0, 100)
            .await
            .unwrap()
            .len(),
        1
    );
    let effects = reopened
        .list_effects(&key("child"), None, 100)
        .await
        .unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].attempt, 0);
    assert!(matches!(
        reopened
            .admit_conditionally(request(&reopened, &policy, "after-revoke").await)
            .await,
        Err(StoreError::Conflict)
    ));
    assert_absent(&reopened, &key("after-revoke")).await;
}

#[tokio::test]
async fn conditional_race_process_child() {
    let Ok(directory) = std::env::var("TOOLKIT_CONDITIONAL_RACE_DIR") else {
        return;
    };
    let directory = std::path::Path::new(&directory);
    let mode = std::env::var("TOOLKIT_CONDITIONAL_RACE_MODE").unwrap();
    let store = SqliteStore::open(directory.join("store.sqlite")).unwrap();
    let policy: StoredRecord =
        serde_json::from_slice(&std::fs::read(directory.join("policy.json")).unwrap()).unwrap();
    let prepared = request(&store, &policy, "child").await;
    std::fs::write(directory.join(format!("ready-{mode}")), b"ready").unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !directory.join("go").exists() {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    if mode == "revoke" {
        store.compare_and_swap(transition(&policy)).await.unwrap();
        return;
    }
    assert_eq!(mode, "publish");
    match store.admit_conditionally(prepared).await {
        Ok(AdmissionOutcome::Created(_)) => std::process::exit(73),
        Err(StoreError::Conflict) => std::process::exit(74),
        outcome => panic!("unexpected race outcome: {outcome:?}"),
    }
}

#[tokio::test]
async fn competing_processes_linearize_publication_against_actual_owner_revocation() {
    for _ in 0..4 {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.sqlite");
        let store = SqliteStore::open(&path).unwrap();
        let policy = owner(&store, "policy").await;
        std::fs::write(
            directory.path().join("policy.json"),
            serde_json::to_vec(&policy).unwrap(),
        )
        .unwrap();
        let spawn = |mode| {
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "conditional_race_process_child", "--nocapture"])
                .env("TOOLKIT_CONDITIONAL_RACE_DIR", directory.path())
                .env("TOOLKIT_CONDITIONAL_RACE_MODE", mode)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        };
        let publisher = spawn("publish");
        let revoker = spawn("revoke");
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !directory.path().join("ready-publish").exists()
                || !directory.path().join("ready-revoke").exists()
            {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        std::fs::write(directory.path().join("go"), b"go").unwrap();
        let publication = publisher.wait_with_output().unwrap();
        let revocation = revoker.wait_with_output().unwrap();
        assert!(
            revocation.status.success(),
            "{}",
            String::from_utf8_lossy(&revocation.stderr)
        );
        match publication.status.code() {
            Some(73) => {
                // Global event rowids witness the serialization order of the
                // actual transactions, not scheduling guesses in the parent.
                let connection = rusqlite::Connection::open(&path).unwrap();
                let rowid = |id: &str, kind: &str| {
                    connection.query_row(
                    "SELECT rowid FROM toolkit_events WHERE realm='realm' AND owner='owner' AND record_id=?1 AND kind=?2",
                    rusqlite::params![id, kind], |r| r.get::<_, i64>(0)).unwrap()
                };
                assert!(rowid("child", "published") < rowid("policy", "revoked"));
                assert_eq!(
                    store
                        .list_effects(&key("child"), None, 100)
                        .await
                        .unwrap()
                        .len(),
                    1
                );
            }
            Some(74) => assert_absent(&store, &key("child")).await,
            status => panic!(
                "publication status {status:?}: {}",
                String::from_utf8_lossy(&publication.stderr)
            ),
        }
        assert_eq!(store.get(&policy.key).await.unwrap().unwrap().revision, 2);
    }
}

#[tokio::test]
async fn both_admission_apis_share_identity_and_replay_returns_current_projection() {
    for conditional_first in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.sqlite");
        let store = SqliteStore::open(&path).unwrap();
        let policy = owner(&store, "policy").await;
        let different = owner(&store, "different-policy").await;
        let original = request(&store, &policy, "child").await;
        let admitted = if conditional_first {
            store.admit_conditionally(original.clone()).await.unwrap()
        } else {
            store.admit(original.admission.clone()).await.unwrap()
        };
        let current = store
            .compare_and_swap(transition(admitted.record()))
            .await
            .unwrap();
        store.compare_and_swap(transition(&policy)).await.unwrap();
        let mut replay = original.clone();
        replay.conditions = vec![condition(&different)];
        replay.before_ms = 1;
        drop(store);
        let store = SqliteStore::open(path).unwrap();
        for result in [
            store.admit(original.admission.clone()).await.unwrap(),
            store.admit_conditionally(replay.clone()).await.unwrap(),
        ] {
            assert!(matches!(result, AdmissionOutcome::Existing(_)));
            assert_eq!(result.record(), &current);
        }
        // All initial payload, effect and target identity conflicts survive
        // crossing API boundaries and later mutation of the admitted record.
        for mutation in 0..3 {
            let mut changed = original.admission.clone();
            match mutation {
                0 => changed.data = data(7),
                1 => changed.effects[0].request = data(7),
                2 => changed.key.id = "another-child".into(),
                _ => unreachable!(),
            }
            assert!(matches!(
                store.admit(changed.clone()).await,
                Err(StoreError::AdmissionConflict)
            ));
            let mut conditional = replay.clone();
            conditional.admission = changed;
            assert!(matches!(
                store.admit_conditionally(conditional).await,
                Err(StoreError::AdmissionConflict)
            ));
        }
        assert_eq!(
            store.read_events(&current.key, 0, 100).await.unwrap().len(),
            2
        );
        let effects = store.list_effects(&current.key, None, 100).await.unwrap();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].attempt, 0);
        assert_absent(&store, &key("another-child")).await;
    }
}
