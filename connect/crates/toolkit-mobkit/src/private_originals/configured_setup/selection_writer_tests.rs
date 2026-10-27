//! Real source selections serialize through the existing writer. Only the
//! canonical SQLite CAS is paused; no fixture grabs the writer itself.
use super::*;
use std::sync::{OnceLock, atomic::{AtomicUsize, Ordering}};
use toolkit_store::*;

struct HeldSelectionWrite {
    inner: Arc<dyn ToolkitStore>,
    hold: std::sync::Mutex<Option<(StoreKey, bool)>>,
    observed: OnceLock<StoreKey>,
    reads: AtomicUsize,
    selections: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}
impl HeldSelectionWrite {
    fn new(inner: Arc<dyn ToolkitStore>) -> Self {
        Self {
            inner, hold: std::sync::Mutex::new(None), observed: OnceLock::new(),
            reads: AtomicUsize::new(0), selections: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(), release: tokio::sync::Semaphore::new(0),
        }
    }
}
#[async_trait::async_trait]
impl ToolkitStore for HeldSelectionWrite {
    fn capabilities(&self) -> StoreCapabilities { self.inner.capabilities() }
    async fn now_ms(&self) -> StoreResult<i64> { self.inner.now_ms().await }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> { self.inner.admit(request).await }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        if self.observed.get() == Some(key) { self.reads.fetch_add(1, Ordering::SeqCst); }
        self.inner.get(key).await
    }
    async fn list_records(&self, scope: &StoreScope, after: Option<&str>, limit: u32) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        let selected = !transition.data.body["configured_setup"].is_null();
        if self.observed.get() == Some(&transition.key) && selected {
            self.selections.fetch_add(1, Ordering::SeqCst);
        }
        let hold = {
            let mut pending = self.hold.lock().unwrap();
            let matches = pending.as_ref().is_some_and(|(key, cancelled)|
                key == &transition.key && if *cancelled {
                    transition.data.body["operation_cancelled"] == true
                } else { selected && transition.data.body["final_association"].is_null() });
            if matches { pending.take(); }
            matches
        };
        if hold {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        self.inner.compare_and_swap(transition).await
    }
    async fn claim_record(&self, key: &StoreKey, revision: u64, worker: &str, ttl: u64) -> StoreResult<RecordLease> {
        self.inner.claim_record(key, revision, worker, ttl).await
    }
    async fn renew_record_claim(&self, lease: &RecordLease, ttl: u64) -> StoreResult<RecordLease> {
        self.inner.renew_record_claim(lease, ttl).await
    }
    async fn recover_record_claim(&self, prior: &RecordLease) -> StoreResult<RecordLease> {
        self.inner.recover_record_claim(prior).await
    }
    async fn release_record_claim(&self, lease: &RecordLease) -> StoreResult<()> {
        self.inner.release_record_claim(lease).await
    }
    async fn read_events(&self, key: &StoreKey, after: u64, limit: u32) -> StoreResult<Vec<StoredEvent>> {
        self.inner.read_events(key, after, limit).await
    }
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> { self.inner.get_effect(key).await }
    async fn list_effects(&self, key: &StoreKey, after: Option<&str>, limit: u32) -> StoreResult<Vec<StoredEffect>> {
        self.inner.list_effects(key, after, limit).await
    }
    async fn claim_effect(&self, key: &EffectKey, revision: u64, worker: &str, ttl: u64) -> StoreResult<EffectClaimOutcome> {
        self.inner.claim_effect(key, revision, worker, ttl).await
    }
    async fn acknowledge_effect(&self, lease: &EffectLease, outcome: EffectOutcome) -> StoreResult<StoredEffect> {
        self.inner.acknowledge_effect(lease, outcome).await
    }
    async fn claim_reconciliation(&self, key: &EffectKey, worker: &str, ttl: u64) -> StoreResult<StoredEffect> {
        self.inner.claim_reconciliation(key, worker, ttl).await
    }
    async fn reconcile_effect(&self, lease: &EffectLease, resolution: Reconciliation) -> StoreResult<StoredEffect> {
        self.inner.reconcile_effect(lease, resolution).await
    }
}

#[derive(Clone, Copy)]
enum WaitingSelection { Both, DropQueued, CancellationFirst }

async fn exercise(mode: WaitingSelection) {
    const WAIT: std::time::Duration = std::time::Duration::from_secs(30);
    let captured_store = Arc::new(OnceLock::<Arc<HeldSelectionWrite>>::new());
    let capture = captured_store.clone();
    let fixture = Box::pin(ContinuationFixture::for_service_input_with_source(Box::new(move |inner| {
        let selected = Arc::new(HeldSelectionWrite::new(inner));
        assert!(capture.set(selected.clone()).is_ok());
        selected
    }))).await;
    let store = captured_store.get().unwrap().clone();
    // Keep the actual entered store operation outside the observer timeout.
    // Cleanup releases its one pause and joins it before closing native owners.
    let mut holder = None;
    let result = AssertUnwindSafe(tokio::time::timeout(WAIT, Box::pin(async {
        let a = captured(&fixture, "selection-writer-a").await;
        let b = captured(&fixture, "selection-writer-b").await;
        assert!(a.original != b.original);
        let a_session = fixture.source.prepare_local_controller_setup(a.clone()).await.unwrap().session_id().clone();
        let b_session = fixture.source.prepare_local_controller_setup(b.clone()).await.unwrap().session_id().clone();
        let a_selected = capability(&fixture, "selection-writer-a");
        let b_selected = capability(&fixture, "selection-writer-b");
        let a_key = fixture.source.key(&a.original).unwrap();
        let b_key = fixture.source.key(&b.original).unwrap();
        let before_b = fixture.store.get(&b_key).await.unwrap().unwrap();
        assert!(fixture.source.decode_record(&before_b, &b.original).unwrap().configured_setup.is_none());
        assert!(store.observed.set(b_key.clone()).is_ok());
        *store.hold.lock().unwrap() = Some(match mode {
            WaitingSelection::CancellationFirst => (b_key.clone(), true),
            _ => (a_key.clone(), false),
        });
        holder = Some(tokio::spawn({
            let source = fixture.source.clone();
            let reference = if matches!(mode, WaitingSelection::CancellationFirst) { b.original.clone() } else { a.original.clone() };
            let session = a_session.clone();
            let selected = a_selected.clone();
            async move {
                if matches!(mode, WaitingSelection::CancellationFirst) {
                    source.record_operation_cancelled(&reference).await
                } else {
                    source.retain_configured_setup(&reference, &session, selected).await.map(|_| ())
                }
            }
        }));
        store.entered.notified().await;
        let reads_before_wait = store.reads.load(Ordering::SeqCst);
        let waiting = fixture.source.owner.observe_source_writer_wait_for_test();
        let mut queued = Some(Box::pin(fixture.source.retain_configured_setup(
            &b.original, &b_session, b_selected.clone(),
        )));
        tokio::select! {
            entered = waiting => entered.unwrap(),
            result = queued.as_mut().unwrap().as_mut() => {
                assert!(result.is_ok(), "selection refused solely because another source write was entered");
                panic!("selection returned before the held canonical CAS settled");
            }
        }
        assert!(matches!(futures::poll!(queued.as_mut().unwrap().as_mut()), std::task::Poll::Pending));
        assert_eq!(store.reads.load(Ordering::SeqCst), reads_before_wait);
        assert_eq!(store.selections.load(Ordering::SeqCst), 0);
        assert!(fixture.store.get(&b_key).await.unwrap().unwrap() == before_b);
        if matches!(mode, WaitingSelection::DropQueued) { drop(queued.take()); }
        store.release.add_permits(1);
        let held = holder.as_mut().unwrap().await;
        drop(holder.take());
        held.unwrap().unwrap();
        match mode {
            WaitingSelection::Both => {
                let selected = queued.as_mut().unwrap().as_mut().await.unwrap();
                assert!(selected.capability == b_selected && selected.session_id == b_session);
                assert_eq!(store.selections.load(Ordering::SeqCst), 1);
            }
            WaitingSelection::DropQueued => {
                assert_eq!(store.reads.load(Ordering::SeqCst), reads_before_wait);
                assert_eq!(store.selections.load(Ordering::SeqCst), 0);
                assert!(fixture.store.get(&b_key).await.unwrap().unwrap() == before_b);
            }
            WaitingSelection::CancellationFirst => {
                assert!(matches!(queued.as_mut().unwrap().as_mut().await, Err(PrivateOriginalError::Denied)));
                assert_eq!(store.reads.load(Ordering::SeqCst), reads_before_wait);
                assert_eq!(store.selections.load(Ordering::SeqCst), 0);
                let cancelled = fixture.store.get(&b_key).await.unwrap().unwrap();
                let cancelled = fixture.source.decode_record(&cancelled, &b.original).unwrap();
                assert!(cancelled.operation_cancelled && cancelled.configured_setup.is_none());
                // Cancellation is source-specific. Another exact source still
                // selects normally after the entered cancellation finishes.
                fixture.source.retain_configured_setup(&a.original, &a_session, a_selected.clone()).await.unwrap();
            }
        }
        drop(queued);
        let saved_a = fixture.store.get(&a_key).await.unwrap().unwrap();
        let saved_a = fixture.source.decode_record(&saved_a, &a.original).unwrap();
        assert!(saved_a.configured_setup.as_ref().is_some_and(|selected|
            selected.capability == a_selected && selected.session_id == a_session));
        let saved_b = fixture.store.get(&b_key).await.unwrap().unwrap();
        let saved_b = fixture.source.decode_record(&saved_b, &b.original).unwrap();
        assert!(saved_a.final_association.is_none() && saved_b.final_association.is_none());
        if matches!(mode, WaitingSelection::Both) {
            assert!(saved_b.configured_setup.as_ref().is_some_and(|selected|
                selected.capability == b_selected && selected.session_id == b_session));
        }
        for original in [&a.original, &b.original] {
            assert!(fixture.source.owner.grants().issued_reference(
                &fixture.source.configured_child_id(original).unwrap(),
            ).unwrap().is_none());
        }
        assert_eq!(fixture.model_requests(), 0);
    }))).catch_unwind().await;
    store.release.add_permits(1);
    let held = match holder {
        Some(handle) => Some(handle.await),
        None => None,
    };
    let closed = fixture.shutdown().await;
    assert!(closed.is_ok(), "native cleanup failed: {closed:?}");
    if let Some(held) = held { assert!(matches!(held, Ok(Ok(()))), "entered source write did not settle"); }
    match result {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("real configured-selection control timed out"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn distinct_configured_originals_wait_for_the_entered_source_cas() {
    exercise(WaitingSelection::Both).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_queued_configured_selection_never_enters_its_callback() {
    exercise(WaitingSelection::DropQueued).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_selection_observes_cancellation_that_won_before_acquisition() {
    exercise(WaitingSelection::CancellationFirst).await;
}
