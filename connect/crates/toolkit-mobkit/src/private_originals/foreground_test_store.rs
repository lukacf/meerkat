//! One-shot callback-write faults over the fixture's selected SQLite owner.
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
};
use toolkit_store::*;

const NEUTRAL: u8 = 0;
const ASSOCIATION_NO_COMMIT: u8 = 1;
const ATTEMPT_LOST_REPLY: u8 = 2;

pub(super) struct ForegroundTestStore {
    inner: Arc<dyn ToolkitStore>,
    mode: AtomicU8,
    pub(super) association_refusals: AtomicUsize,
    pub(super) attempt_commits: AtomicUsize,
}

impl ForegroundTestStore {
    pub(super) fn new(inner: Arc<dyn ToolkitStore>) -> Self {
        Self {
            inner,
            mode: AtomicU8::new(NEUTRAL),
            association_refusals: AtomicUsize::new(0),
            attempt_commits: AtomicUsize::new(0),
        }
    }

    pub(super) fn arm_association_no_commit(&self) {
        self.arm(ASSOCIATION_NO_COMMIT);
    }

    pub(super) fn arm_attempt_lost_reply(&self) {
        self.arm(ATTEMPT_LOST_REPLY);
    }

    fn arm(&self, mode: u8) {
        assert!(self.mode.compare_exchange(
            NEUTRAL, mode, Ordering::SeqCst, Ordering::SeqCst,
        ).is_ok(), "a foreground fault is already armed");
    }
}

#[async_trait::async_trait]
impl ToolkitStore for ForegroundTestStore {
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
        if transition.data.schema == crate::private_originals::SOURCE_SCHEMA
            && !transition.data.body["final_association"].is_null()
        {
            // The fixture arms immediately before the relevant callback. Only
            // that callback's first matching write consumes the fault.
            let mode = if transition.data.body["custody_attempt"].is_null() {
                ASSOCIATION_NO_COMMIT
            } else {
                ATTEMPT_LOST_REPLY
            };
            if self.mode.compare_exchange(
                mode, NEUTRAL, Ordering::SeqCst, Ordering::SeqCst,
            ).is_ok() {
                if mode == ASSOCIATION_NO_COMMIT {
                    self.association_refusals.fetch_add(1, Ordering::SeqCst);
                    return Err(StoreError::OwnerUnavailable);
                }
                self.inner.compare_and_swap(transition).await?;
                self.attempt_commits.fetch_add(1, Ordering::SeqCst);
                return Err(StoreError::OwnerUnavailable);
            }
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
