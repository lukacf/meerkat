//! Pause one actual canonical publication CAS; all other store operations delegate.
use super::*;
use toolkit_store::*;
pub(super) struct HeldPublicationWrite {
    inner: Arc<dyn ToolkitStore>,
    hold: AtomicBool,
    event: &'static str,
    pub(super) entered: tokio::sync::Notify,
    pub(super) release: tokio::sync::Semaphore,
    pub(super) entered_count: AtomicUsize,
    pub(super) returned_count: AtomicUsize,
}
impl HeldPublicationWrite {
    pub(super) fn new(inner: Arc<dyn ToolkitStore>) -> Self {
        Self::for_event(inner, "member-route-prepared")
    }
    pub(super) fn for_event(inner: Arc<dyn ToolkitStore>, event: &'static str) -> Self {
        Self {
            inner,
            event,
            hold: AtomicBool::new(true),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
            entered_count: AtomicUsize::new(0),
            returned_count: AtomicUsize::new(0),
        }
    }
}
#[async_trait::async_trait]
impl ToolkitStore for HeldPublicationWrite {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        self.inner.admit(request).await
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        self.inner.get(key).await
    }
    async fn list_records(
        &self,
        scope: &StoreScope,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        if transition
            .events
            .iter()
            .any(|event| event.kind == self.event)
            && self.hold.swap(false, Ordering::SeqCst)
        {
            self.entered_count.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            let result = self.inner.compare_and_swap(transition).await;
            self.returned_count.fetch_add(1, Ordering::SeqCst);
            return result;
        }
        self.inner.compare_and_swap(transition).await
    }
    async fn claim_record(
        &self,
        key: &StoreKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<RecordLease> {
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
    async fn read_events(
        &self,
        key: &StoreKey,
        after: u64,
        limit: u32,
    ) -> StoreResult<Vec<StoredEvent>> {
        self.inner.read_events(key, after, limit).await
    }
    async fn get_effect(&self, key: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        self.inner.get_effect(key).await
    }
    async fn list_effects(
        &self,
        key: &StoreKey,
        after: Option<&str>,
        limit: u32,
    ) -> StoreResult<Vec<StoredEffect>> {
        self.inner.list_effects(key, after, limit).await
    }
    async fn claim_effect(
        &self,
        key: &EffectKey,
        revision: u64,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<EffectClaimOutcome> {
        self.inner.claim_effect(key, revision, worker, ttl).await
    }
    async fn acknowledge_effect(
        &self,
        lease: &EffectLease,
        outcome: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        self.inner.acknowledge_effect(lease, outcome).await
    }
    async fn claim_reconciliation(
        &self,
        key: &EffectKey,
        worker: &str,
        ttl: u64,
    ) -> StoreResult<StoredEffect> {
        self.inner.claim_reconciliation(key, worker, ttl).await
    }
    async fn reconcile_effect(
        &self,
        lease: &EffectLease,
        resolution: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        self.inner.reconcile_effect(lease, resolution).await
    }
}
