//! Capability inspection is required during construction; operations are forbidden.
use super::*;

pub(super) struct NoOperations(pub(super) Arc<SqliteStore>);

#[async_trait]
impl ToolkitStore for NoOperations {
    fn capabilities(&self) -> StoreCapabilities {
        self.0.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        panic!("unexpected store clock access")
    }
    async fn admit(&self, _: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        panic!("unexpected store admission")
    }
    async fn get(&self, _: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        panic!("unexpected store lookup")
    }
    async fn list_records(
        &self,
        _: &StoreScope,
        _: Option<&str>,
        _: u32,
    ) -> StoreResult<Vec<StoredRecord>> {
        panic!("unexpected record listing")
    }
    async fn compare_and_swap(&self, _: Transition) -> StoreResult<StoredRecord> {
        panic!("unexpected record mutation")
    }
    async fn claim_record(
        &self,
        _: &StoreKey,
        _: u64,
        _: &str,
        _: u64,
    ) -> StoreResult<RecordLease> {
        panic!("unexpected record claim")
    }
    async fn renew_record_claim(&self, _: &RecordLease, _: u64) -> StoreResult<RecordLease> {
        panic!("unexpected record claim renewal")
    }
    async fn recover_record_claim(&self, _: &RecordLease) -> StoreResult<RecordLease> {
        panic!("unexpected record claim recovery")
    }
    async fn release_record_claim(&self, _: &RecordLease) -> StoreResult<()> {
        panic!("unexpected record claim release")
    }
    async fn read_events(&self, _: &StoreKey, _: u64, _: u32) -> StoreResult<Vec<StoredEvent>> {
        panic!("unexpected event read")
    }
    async fn get_effect(&self, _: &EffectKey) -> StoreResult<Option<StoredEffect>> {
        panic!("unexpected effect lookup")
    }
    async fn list_effects(
        &self,
        _: &StoreKey,
        _: Option<&str>,
        _: u32,
    ) -> StoreResult<Vec<StoredEffect>> {
        panic!("unexpected effect listing")
    }
    async fn claim_effect(
        &self,
        _: &EffectKey,
        _: u64,
        _: &str,
        _: u64,
    ) -> StoreResult<EffectClaimOutcome> {
        panic!("unexpected effect claim")
    }
    async fn acknowledge_effect(
        &self,
        _: &EffectLease,
        _: EffectOutcome,
    ) -> StoreResult<StoredEffect> {
        panic!("unexpected effect acknowledgment")
    }
    async fn claim_reconciliation(
        &self,
        _: &EffectKey,
        _: &str,
        _: u64,
    ) -> StoreResult<StoredEffect> {
        panic!("unexpected reconciliation claim")
    }
    async fn reconcile_effect(
        &self,
        _: &EffectLease,
        _: Reconciliation,
    ) -> StoreResult<StoredEffect> {
        panic!("unexpected effect reconciliation")
    }
}
