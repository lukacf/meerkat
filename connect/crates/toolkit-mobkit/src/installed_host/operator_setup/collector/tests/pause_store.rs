//! Controlled wait around real SQLite admission; all store behavior is forwarded.
use super::*;
use toolkit_store::*;
struct PauseStore {
    inner: Arc<dyn ToolkitStore>,
    after_commit: bool,
    armed: std::sync::atomic::AtomicBool,
    cleanup_failure: std::sync::atomic::AtomicU8,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl PauseStore {
    fn new(inner: Arc<dyn ToolkitStore>, after_commit: bool) -> Arc<Self> {
        Arc::new(Self {
            inner,
            after_commit,
            armed: std::sync::atomic::AtomicBool::new(true),
            cleanup_failure: std::sync::atomic::AtomicU8::new(0),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        })
    }
}
#[async_trait::async_trait]
impl ToolkitStore for PauseStore {
    fn capabilities(&self) -> StoreCapabilities {
        self.inner.capabilities()
    }
    async fn now_ms(&self) -> StoreResult<i64> {
        self.inner.now_ms().await
    }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        let pause = request.data.schema == "toolkit.collector-setup"
            && request.data.version == 1
            && self.armed.swap(false, std::sync::atomic::Ordering::SeqCst);
        if pause && !self.after_commit {
            self.entered.notify_one();
            self.release.notified().await;
        }
        let result = self.inner.admit(request).await;
        if pause && self.after_commit {
            self.entered.notify_one();
            self.release.notified().await;
        }
        result
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
        let failure = if transition.data.schema == DECISION_SCHEMA
            && transition
                .data
                .body
                .get("cleanup")
                .is_some_and(|value| !value.is_null())
        {
            self.cleanup_failure
                .swap(0, std::sync::atomic::Ordering::SeqCst)
        } else {
            0
        };
        if failure == 1 {
            return Err(StoreError::OwnerUnavailable);
        }
        let result = self.inner.compare_and_swap(transition).await;
        if failure == 2 && result.is_ok() {
            return Err(StoreError::OwnerUnavailable);
        }
        result
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

#[tokio::test]
async fn revoke_overlapping_enrollment_before_or_after_real_store_commit_never_reports_active() {
    for (after_commit, cleanup_available) in [(false, true), (true, true), (true, false)] {
        let (_directory, selection) = crate::installed_host::tests::fixture();
        let mut owner = OperatorSetup::open(
            selection,
            OperatorOwnerStartup::fresh("collector-overlap", 1).unwrap(),
        )
        .unwrap();
        let probe = PauseStore::new(owner.originals.clone(), after_commit);
        owner.originals = probe.clone();
        let scope = StoreScope {
            realm: owner.native.realm.to_string(),
            owner: owner.lifetime.selected_policy().deployment.to_string(),
        };
        let originals =
            PrivateOriginals::prepare(scope.clone(), owner.originals.clone(), owner.source.clone())
                .await
                .unwrap();
        owner
            .source
            .select_installed_pair_recovery(owner.originals.as_ref(), &scope)
            .unwrap();
        let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
        if cleanup_available {
            originals.bind_machine(machine.clone()).unwrap();
        }
        let review = review(&owner);
        let saved = owner
            .retain_collector_decision(&decision(&review))
            .await
            .unwrap();
        let issued = owner
            .issue_collector_decision(&originals, saved, &|| true)
            .await
            .unwrap();
        let decision = owner.decode_collector_decision(&issued).unwrap();
        let record = owner.collector_record(&issued, &decision).unwrap();
        let publishing = originals.persist_collector_setup(record.clone());
        let cancelling = async {
            probe.entered.notified().await;
            let result = owner
                .revoke_collector_enrollment(&originals, review.id())
                .await;
            probe.release.notify_one();
            result
        };
        let (published, cancelled) = tokio::join!(publishing, cancelling);
        if cleanup_available {
            assert!(cancelled.is_ok(), "{cancelled:?}");
        } else {
            assert!(
                matches!(cancelled, Err(CollectorSetupError::PendingReconciliation)),
                "{cancelled:?}"
            );
        }
        assert!(
            matches!(
                published,
                Err(CollectorSetupError::Denied | CollectorSetupError::SetupRevoked)
            ),
            "{published:?}"
        );
        assert!(matches!(
            originals.collector_setup_not_revoked(review.id()).await,
            Err(CollectorSetupError::SetupRevoked)
        ));
        let grant = owner.source.grants().resolve_lineage(
            &record.lineage,
            &record.association.logical_executor,
            None,
        );
        if cleanup_available {
            assert!(grant.is_err());
        } else {
            // The native grant remains live, so the canonical enrollment read,
            // not successful native cleanup, must refuse the delayed reply.
            grant.unwrap().check_current().unwrap();
        }
        let retained = owner.originals.get(&issued.key).await.unwrap().unwrap();
        assert!(matches!(
            owner
                .issue_collector_decision(&originals, retained, &|| true)
                .await,
            Err(CollectorSetupError::SetupRevoked)
        ));
        drop(originals);
        drop(machine);
        owner.finish().await.unwrap();
    }
}

#[tokio::test]
async fn native_revoke_success_with_missing_or_lost_receipt_reconciles_exactly() {
    for failure in [1, 2] {
        let (_directory, selection) = crate::installed_host::tests::fixture();
        let mut owner = OperatorSetup::open(
            selection,
            OperatorOwnerStartup::fresh("collector-cleanup-retry", 1).unwrap(),
        )
        .unwrap();
        let probe = PauseStore::new(owner.originals.clone(), false);
        owner.originals = probe.clone();
        let scope = StoreScope {
            realm: owner.native.realm.to_string(),
            owner: owner.lifetime.selected_policy().deployment.to_string(),
        };
        let originals =
            PrivateOriginals::prepare(scope.clone(), owner.originals.clone(), owner.source.clone())
                .await
                .unwrap();
        owner
            .source
            .select_installed_pair_recovery(owner.originals.as_ref(), &scope)
            .unwrap();
        let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
        originals.bind_machine(machine.clone()).unwrap();
        let review = review(&owner);
        let saved = owner
            .retain_collector_decision(&decision(&review))
            .await
            .unwrap();
        let issued = owner
            .issue_collector_decision(&originals, saved, &|| true)
            .await
            .unwrap();
        let decision = owner.decode_collector_decision(&issued).unwrap();
        let record = owner.collector_record(&issued, &decision).unwrap();
        probe
            .cleanup_failure
            .store(failure, std::sync::atomic::Ordering::SeqCst);
        assert!(matches!(
            owner
                .revoke_collector_enrollment(&originals, review.id())
                .await,
            Err(CollectorSetupError::PendingReconciliation)
        ));
        assert!(
            owner
                .source
                .grants()
                .resolve_lineage(&record.lineage, &record.association.logical_executor, None,)
                .is_err()
        );
        let saved = owner.originals.get(&issued.key).await.unwrap().unwrap();
        let cancelled = owner.decode_collector_decision(&saved).unwrap();
        assert!(cancelled.cancelled);
        assert_eq!(cancelled.cleanup.is_some(), failure == 2);
        // Without a receipt this calls the real native revoke again. Core's
        // RevokeAlready disposition succeeds without resurrecting the grant.
        owner
            .revoke_collector_enrollment(&originals, review.id())
            .await
            .unwrap();
        let saved = owner.originals.get(&issued.key).await.unwrap().unwrap();
        let cancelled = owner.decode_collector_decision(&saved).unwrap();
        assert!(
            matches!(cancelled.cleanup, Some(CollectorGrantCleanup::Revoked { lineage }) if lineage == record.lineage)
        );
        assert!(matches!(
            owner
                .issue_collector_decision(&originals, issued, &|| true)
                .await,
            Err(CollectorSetupError::SetupRevoked)
        ));
        drop(originals);
        drop(machine);
        owner.finish().await.unwrap();
    }
}
