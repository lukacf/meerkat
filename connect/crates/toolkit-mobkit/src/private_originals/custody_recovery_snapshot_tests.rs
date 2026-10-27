//! Actual installed enrollment during a retained cold census. The source task
//! must visit the new foreground handoff after stop, before joining its owner.
use super::*;
use std::sync::{OnceLock, atomic::{AtomicBool, AtomicUsize, Ordering}};
use toolkit_store::*;
use crate::original_ingress::custody_attempt::{CustodyAttempt, CustodyDecision};
use crate::private_originals::custody_recovery::HandoffPhase;
use meerkat_runtime::{LogicalRuntimeId, input_state::verify_exact_replay};

type InstalledFixture = FirstFixture<meerkat::PersistentSessionService<FactoryAgentBuilder>>;

struct RecoveryStore {
    inner: Arc<dyn ToolkitStore>,
    scope: OnceLock<StoreScope>,
    terminal_mode: AtomicUsize,
    terminal_calls: AtomicUsize,
    terminal_returns: AtomicUsize,
    terminal_entered: tokio::sync::Notify,
    terminal_release: tokio::sync::Semaphore,
    reads: AtomicUsize,
    armed: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    started: AtomicUsize,
    completed: AtomicUsize,
    cancelled: AtomicUsize,
}
impl RecoveryStore {
    fn new(inner: Arc<dyn ToolkitStore>) -> Self {
        Self { inner, scope: OnceLock::new(), terminal_mode: AtomicUsize::new(0),
            terminal_calls: AtomicUsize::new(0), terminal_returns: AtomicUsize::new(0),
            terminal_entered: tokio::sync::Notify::new(), terminal_release: tokio::sync::Semaphore::new(0),
            reads: AtomicUsize::new(0), armed: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(), release: tokio::sync::Semaphore::new(0),
            started: AtomicUsize::new(0), completed: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0) }
    }
    fn arm(&self) {
        assert!(!self.armed.swap(true, Ordering::SeqCst));
    }
}
struct EnteredRead<'a> { store: &'a RecoveryStore, complete: bool }
impl Drop for EnteredRead<'_> {
    fn drop(&mut self) {
        if !self.complete { self.store.cancelled.fetch_add(1, Ordering::SeqCst); }
    }
}
#[async_trait::async_trait]
impl ToolkitStore for RecoveryStore {
    fn capabilities(&self) -> StoreCapabilities { self.inner.capabilities() }
    async fn now_ms(&self) -> StoreResult<i64> { self.inner.now_ms().await }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        self.inner.admit(request).await
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get(key).await
    }
    async fn list_records(&self, scope: &StoreScope, after: Option<&str>, limit: u32)
        -> StoreResult<Vec<StoredRecord>>
    {
        if self.scope.get() == Some(scope) && self.armed.swap(false, Ordering::SeqCst) {
            assert!(after.is_none());
            assert_eq!(limit, 32, "observe the first bounded canonical census read");
            self.started.fetch_add(1, Ordering::SeqCst);
            let mut entered = EnteredRead { store: self, complete: false };
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            let result = self.inner.list_records(scope, after, limit).await;
            self.completed.fetch_add(1, Ordering::SeqCst);
            entered.complete = true;
            result
        } else { self.inner.list_records(scope, after, limit).await }
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        let mode = self.terminal_mode.load(Ordering::SeqCst);
        let terminal = transition.data.schema == "toolkit.authenticated-source-event"
            && transition.data.version == 4
            && transition.data.body["custody_attempt"]["decision"].as_str()
                .is_some_and(|decision| decision == "promoted" || decision == "withdrawn");
        if mode != 0 && terminal {
            self.terminal_calls.fetch_add(1, Ordering::SeqCst);
            let mut entered = EnteredRead { store: self, complete: false };
            if mode == 2 {
                self.terminal_entered.notify_one();
                self.terminal_release.acquire().await.unwrap().forget();
            }
            self.terminal_returns.fetch_add(1, Ordering::SeqCst);
            entered.complete = true;
            return Err(StoreError::OwnerUnavailable);
        }
        self.inner.compare_and_swap(transition).await
    }
    async fn claim_record(&self, key: &StoreKey, revision: u64, worker: &str, ttl: u64)
        -> StoreResult<RecordLease>
    { self.inner.claim_record(key, revision, worker, ttl).await }
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
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        self.inner.get_effect(key).await
    }
    async fn list_effects(&self, key: &StoreKey, after: Option<&str>, limit: u32)
        -> StoreResult<Vec<StoredEffect>>
    { self.inner.list_effects(key, after, limit).await }
    async fn claim_effect(&self, key: &EffectKey, revision: u64, worker: &str, ttl: u64)
        -> StoreResult<EffectClaimOutcome>
    { self.inner.claim_effect(key, revision, worker, ttl).await }
    async fn acknowledge_effect(&self, lease: &EffectLease, outcome: EffectOutcome)
        -> StoreResult<StoredEffect>
    { self.inner.acknowledge_effect(lease, outcome).await }
    async fn claim_reconciliation(&self, key: &EffectKey, worker: &str, ttl: u64)
        -> StoreResult<StoredEffect>
    { self.inner.claim_reconciliation(key, worker, ttl).await }
    async fn reconcile_effect(&self, lease: &EffectLease, resolution: Reconciliation)
        -> StoreResult<StoredEffect>
    { self.inner.reconcile_effect(lease, resolution).await }
}


async fn open_installed() -> (Arc<InstalledFixture>, Arc<dyn meerkat_runtime::RuntimeStore>,
    DurableFirstInput, Arc<crate::installed_host::acquisition::InstalledNativeContinuation>, Arc<RecoveryStore>)
{
    let capture = Arc::new(OnceLock::new());
    let selected = capture.clone();
    let (fixture, history, retained, continuation) = InstalledFixture::construct_persistent_observed_source(
        true, None, None, false, None, None, Some(Box::new(move |inner| {
            let store = Arc::new(RecoveryStore::new(inner));
            assert!(selected.set(store.clone()).is_ok());
            store
        }))).await;
    let store = capture.get().unwrap().clone();
    assert!(store.scope.set(fixture.scope.clone()).is_ok());
    (Arc::new(fixture), history, retained.unwrap(), continuation.unwrap(), store)
}

async fn reopen_installed(retained: DurableFirstInput) -> (Arc<InstalledFixture>,
    Arc<dyn meerkat_runtime::RuntimeStore>, Arc<crate::installed_host::acquisition::InstalledNativeContinuation>, Arc<RecoveryStore>)
{
    let capture = Arc::new(OnceLock::new());
    let selected = capture.clone();
    let (fixture, history, continuation) = InstalledFixture::reopen_durable_continuation_with_source(
        retained, Box::new(move |inner| {
            let store = Arc::new(RecoveryStore::new(inner));
            assert!(selected.set(store.clone()).is_ok());
            store
        })).await;
    let store = capture.get().unwrap().clone();
    assert!(store.scope.set(fixture.scope.clone()).is_ok());
    (Arc::new(fixture), history, continuation, store)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_owner_visits_enrollment_after_its_held_census_snapshot() {
    let (fixture, history, _retained, _continuation, store) = open_installed().await;
    let mut native_close = fixture.retained_close_for_test();
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 4, Box::pin(async {
        assert!(fixture.source.owner.has_installed_pair_recovery().unwrap());
        store.arm();
        fixture.source.start_custody_recovery(
            fixture.continuation_native().continuation_source_announcer()).unwrap();
        store.entered.notified().await;
        assert_eq!(store.completed.load(Ordering::SeqCst), 0);
        // The initial hot snapshot was empty. This real native first input
        // enrolls while that same task still owns its first cold page future.
        let admitted = fixture.submit().await;
        assert!(matches!(&admitted, Ok(PrivateOriginalSubmission::Admitted {
            deduplicated: false, source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
        })));
        fixture.finish(&admitted).await.unwrap();
        let held = fixture.source.retained.lock().unwrap().iter()
            .find(|held| held.reference == *fixture.original_reference()).cloned().unwrap();
        let pending = held.first_custody.lock().unwrap().clone().unwrap();
        assert!(pending.handoff.phase() == HandoffPhase::Accepted);
        let receipt = pending.native_admission.lock().unwrap().clone().unwrap();
        assert_eq!(receipt.session_id, fixture.session);
        assert_eq!(&receipt.input_id, pending.input.id());
        let before = fixture.cold_source_record().await;
        let attempt: CustodyAttempt = serde_json::from_value(before.data.body["custody_attempt"].clone()).unwrap();
        assert!(attempt.decision() == CustodyDecision::Pending);
        assert!(attempt.matches_input(&pending.input).unwrap());
        fixture.source.stop_custody_recovery();
        let mut joined = Box::pin(fixture.source.join_custody_recovery());
        assert!(matches!(futures::poll!(joined.as_mut()), std::task::Poll::Pending));
        assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
        store.release.add_permits(1);
        joined.await.unwrap();
        assert_eq!(store.completed.load(Ordering::SeqCst), 1);
        assert!(pending.handoff.phase() == HandoffPhase::Done);
        let after = fixture.cold_source_record().await;
        let terminal: CustodyAttempt = serde_json::from_value(after.data.body["custody_attempt"].clone()).unwrap();
        assert!(terminal.decision() == CustodyDecision::Promoted);
        assert!(terminal.same_attempt(&attempt));
        assert_eq!(after.revision, before.revision + 1);
        let mut expected = before.data.body.clone();
        expected["custody_attempt"] = after.data.body["custody_attempt"].clone();
        assert_eq!(after.data.body, expected);
        let rows = history.load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session)).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state.input_id, receipt.input_id);
        verify_exact_replay(&rows[0].state, &pending.input).unwrap();
        let reads = store.reads.load(Ordering::SeqCst);
        fixture.source.join_custody_recovery().await.unwrap();
        assert_eq!(store.reads.load(Ordering::SeqCst), reads);
    }))).catch_unwind().await;
    fixture.source.stop_custody_recovery();
    store.release.add_permits(1); store.terminal_release.add_permits(1);
    let (native, source) = tokio::join!(native_close.join(), fixture.source.join_custody_recovery());
    match checked {
        Err(payload) => {
            if native.is_err() || source.is_err() { eprintln!("snapshot cleanup: {native:?}; {source:?}"); }
            std::panic::resume_unwind(payload);
        },
        Ok(result) => {
            assert!(native.is_ok() && source.is_ok(), "snapshot cleanup: {native:?}; {source:?}");
            assert!(result.is_ok(), "actual snapshot/enrollment control timed out");
            assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
        },
    }
}

#[path = "custody_recovery_cold_error_tests.rs"]
mod custody_recovery_cold_error_tests;
