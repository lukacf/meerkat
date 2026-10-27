//! Actual SQLite transactions, including work queued after caller loss.
use super::*;
use std::future::Future;
use std::task::Poll;

fn admission(id: &str) -> AdmissionRequest {
    AdmissionRequest {
        key: StoreKey { realm: "realm".into(), owner: "conditional".into(), id: id.into() },
        admission_key: id.into(),
        data: RecordData { schema: "test.conditional".into(), version: 1, body: serde_json::json!({"state":"open"}) },
        events: vec![], effects: vec![],
    }
}
fn condition(record: &StoredRecord) -> RecordCondition {
    RecordCondition { key: record.key.clone(), revision: record.revision,
        content_digest: record.content_digest.clone(), claim: None, dispatch_claim: None }
}
fn transition(record: &StoredRecord, state: &str) -> Transition {
    let data = RecordData { schema: "test.conditional".into(), version: 1,
        body: serde_json::json!({"state":state}) };
    Transition { key: record.key.clone(), expected_revision: record.revision,
        expected_digest: Some(record.content_digest.clone()), claim: None,
        data: data.clone(), events: vec![EventWrite { kind: state.into(), data: data.clone() }],
        effects: vec![EffectIntent { id: state.into(), kind: "test.obligation".into(), request: data,
            idempotency_key: state.into(), replay_safety: ReplaySafety::ReconcileBeforeRetry }] }
}

// Retains and joins only the real connection-lock holder. Dropping the tested
// async call still leaves SqliteStore's own entered blocking worker intact.
struct HeldConnection {
    release: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl HeldConnection {
    async fn new(store: &SqliteStore) -> Self {
        let connection = store.connection.clone();
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let _guard = connection.lock().unwrap();
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
        });
        let held = Self { release: Some(release_tx), thread: Some(thread) };
        entered_rx.await.unwrap();
        held
    }
    fn finish(mut self) -> bool {
        if let Some(release) = self.release.take() { let _ = release.send(()); }
        self.thread.take().unwrap().join().is_ok()
    }
}
impl Drop for HeldConnection {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() { let _ = release.send(()); }
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

#[tokio::test]
async fn queued_conditional_join_cannot_commit_after_source_cancel_and_caller_drop() {
    for later_join in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("store.sqlite");
        let store = SqliteStore::open(&path).unwrap();
        let canceller = SqliteStore::open_existing(&path).unwrap();
        let source = store.admit(admission("source-a")).await.unwrap().record().clone();
        let prior = if later_join {
            Some(store.admit(admission("workflow")).await.unwrap().record().clone())
        } else { None };
        let deadline = store.now_ms().await.unwrap() + 60_000;
        let held = HeldConnection::new(&store).await;
        let mut future = Box::pin(async {
            if let Some(prior) = &prior {
                store.compare_and_swap_conditionally(ConditionalTransition {
                    transition: transition(prior, "join-a"), conditions: vec![condition(&source)], before_ms: deadline,
                }).await.map(|_| ())
            } else {
                let mut request = admission("workflow");
                request.effects = transition(&source, "join-a").effects;
                store.admit_conditionally(ConditionalAdmission {
                    admission: request, conditions: vec![condition(&source)], before_ms: deadline,
                }).await.map(|_| ())
            }
        });
        // One actual poll enters SqliteStore::with and queues its blocking
        // worker behind the held connection. This does not replace the store.
        let pending = std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx).is_pending())).await;
        let queued_owner = Arc::strong_count(&store.connection) >= 3;
        let cancelled = canceller.compare_and_swap(transition(&source, "cancelled")).await;
        drop(future);
        let holder_joined = held.finish();
        // Observe release of the actual transaction/connection owner, not a
        // join of Tokio's reusable pool thread or ordering of a later read.
        let connection_settled = tokio::time::timeout(Duration::from_secs(5), async {
            while Arc::strong_count(&store.connection) != 1 { tokio::task::yield_now().await; }
        }).await.is_ok();
        assert!(holder_joined && connection_settled);
        assert!(pending && queued_owner);
        assert_eq!(cancelled.unwrap().data.body["state"], "cancelled");
        let target = admission("workflow").key;
        assert_eq!(store.get(&target).await.unwrap(), prior);
        assert!(store.read_events(&target, 0, 16).await.unwrap().is_empty());
        assert!(store.list_effects(&target, None, 16).await.unwrap().is_empty());
    }
}

#[tokio::test]
async fn conditional_transition_checks_target_and_source_and_rolls_back_late_expiry() {
    for target_claim in [false, true] {
    let directory = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(directory.path().join("store.sqlite")).unwrap();
    let source = store.admit(admission("source-a")).await.unwrap().record().clone();
    let prior = store.admit(admission("workflow")).await.unwrap().record().clone();
    let base = store.now_ms().await.unwrap();
    let mut request = ConditionalTransition { transition: transition(&prior, "join-a"),
        conditions: vec![condition(&source)], before_ms: base + 60_000 };
    if target_claim {
        request.transition.claim = Some(store.claim_record(&prior.key, prior.revision, "join-owner", 60_000).await.unwrap());
        request.before_ms = base + 120_000;
    }
    let expiry = request.transition.claim.as_ref().map_or(request.before_ms, |lease| lease.expires_at_ms);
    let mut wrong_digest = request.clone();
    wrong_digest.transition.expected_digest = Some("0".repeat(64));
    assert!(matches!(store.compare_and_swap_conditionally(wrong_digest).await, Err(StoreError::Conflict)));
    // The write and event/effect insert occur before the final clock check.
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let clock = calls.clone();
    store.with(move |connection, _| {
        connection.create_scalar_function("unixepoch", 1, rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |_| {
                let call = clock.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok((if call < 3 { base } else { expiry + 1_000 }) as f64 / 1000.0)
            })?;
        Ok(())
    }).await.unwrap();
    let outcome = store.compare_and_swap_conditionally(request.clone()).await;
    if target_claim { assert!(matches!(outcome, Err(StoreError::Fenced))); }
    else { assert!(matches!(outcome, Err(StoreError::AdmissionExpired))); }
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 4);
    assert_eq!(store.get(&prior.key).await.unwrap().unwrap(), prior);
    assert!(store.read_events(&prior.key, 0, 16).await.unwrap().is_empty());
    assert!(store.list_effects(&prior.key, None, 16).await.unwrap().is_empty());
    // A fresh real backend handle supplies its own ordinary clock.
    drop(store);
    let store = SqliteStore::open_existing(directory.path().join("store.sqlite")).unwrap();
    let committed = store.compare_and_swap_conditionally(request.clone()).await.unwrap();
    assert_eq!(committed.revision, prior.revision + 1);
    assert!(matches!(store.compare_and_swap_conditionally(request).await, Err(StoreError::Conflict)));
    let cancelled = store.compare_and_swap(transition(&source, "cancelled")).await.unwrap();
    assert_ne!(cancelled.content_digest, source.content_digest);
    let next = ConditionalTransition { transition: transition(&committed, "join-b"),
        conditions: vec![condition(&source)], before_ms: base + 60_000 };
    assert!(matches!(store.compare_and_swap_conditionally(next).await, Err(StoreError::Conflict)));
    assert_eq!(store.get(&committed.key).await.unwrap().unwrap(), committed);
    assert_eq!(store.list_effects(&committed.key, None, 16).await.unwrap().len(), 1);
    }
}
