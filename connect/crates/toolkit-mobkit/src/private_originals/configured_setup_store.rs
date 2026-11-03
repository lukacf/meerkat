//! Mechanical storage observation fault over the same canonical SQLite owner.
use super::*;
use std::sync::atomic::AtomicBool;
use toolkit_store::*;

pub(super) struct LostReplyStore {
    pub(super) inner: Arc<dyn ToolkitStore>,
    pub(super) arm: AtomicBool,
    fail_get: Mutex<Option<StoreKey>>,
    pub(super) committed: AtomicBool,
    pub(super) failed_reads: AtomicUsize,
}
impl LostReplyStore {
    pub(super) fn new(inner: Arc<dyn ToolkitStore>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            arm: AtomicBool::new(true),
            fail_get: Mutex::new(None),
            committed: AtomicBool::new(false),
            failed_reads: AtomicUsize::new(0),
        })
    }
}
#[async_trait::async_trait]
impl ToolkitStore for LostReplyStore {
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
        let fail = {
            let mut pending = self.fail_get.lock().unwrap();
            if pending.as_ref() == Some(key) {
                pending.take();
                true
            } else {
                false
            }
        };
        if fail {
            self.failed_reads.fetch_add(1, Ordering::SeqCst);
            return Err(StoreError::OwnerUnavailable);
        }
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
        if transition.data.schema == SOURCE_SCHEMA
            && !transition.data.body["final_association"].is_null()
            && self.arm.swap(false, Ordering::SeqCst)
        {
            let key = transition.key.clone();
            self.inner.compare_and_swap(transition).await?;
            self.committed.store(true, Ordering::SeqCst);
            *self.fail_get.lock().unwrap() = Some(key);
            return Err(StoreError::OwnerUnavailable);
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
