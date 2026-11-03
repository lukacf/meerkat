//! Controlled wait around real SQLite admission; all store behavior is forwarded.
use super::*;
use toolkit_store::*;
/// The collector setup admission versions a probe can hold: the enrollment
/// publication's record and the cancellation's tombstone.
const ENROLLMENT_ADMISSION: u32 = 1;
const TOMBSTONE_ADMISSION: u32 = 2;
/// Bound on each controlled step, so a lost wakeup fails with its step
/// instead of hanging the test.
const OVERLAP_BOUND: std::time::Duration = std::time::Duration::from_secs(30);
struct PauseStore {
    inner: Arc<dyn ToolkitStore>,
    version: u32,
    after_commit: bool,
    armed: std::sync::atomic::AtomicBool,
    cleanup_failure: std::sync::atomic::AtomicU8,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
impl PauseStore {
    fn new(inner: Arc<dyn ToolkitStore>, version: u32, after_commit: bool) -> Arc<Self> {
        Arc::new(Self {
            inner,
            version,
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
            && request.data.version == self.version
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

/// The enrollment publication holds the source writer across its real store
/// admission, here held before or after the commit. A revocation that
/// overlaps it queues on that same writer, so the publication linearizes
/// first and returns the enrollment it committed; the revocation then runs.
/// Once it settles nothing reports the enrollment active: the canonical row
/// is revoked, the published original is cancelled, and an exact retry of
/// the publication is refused.
#[tokio::test]
async fn revoke_overlapping_enrollment_before_or_after_real_store_commit_never_reports_active() {
    use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, OriginalDisposition};
    for (after_commit, cleanup_available) in [(false, true), (true, true), (true, false)] {
        let (_directory, selection) = crate::installed_host::tests::fixture();
        let mut owner = OperatorSetup::open(
            selection,
            OperatorOwnerStartup::fresh("collector-overlap", 1).unwrap(),
        )
        .unwrap();
        let probe = PauseStore::new(owner.originals.clone(), ENROLLMENT_ADMISSION, after_commit);
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
        let namespace = &record.association.ingress_namespace;
        let original = &record.association.original_work;
        // Observes the first source writer acquisition that has to wait.
        let mut queued = owner.source.observe_source_writer_wait_for_test();
        let (published, cancelled) = {
            let publishing = originals.persist_collector_setup(record.clone());
            let cancelling = owner.revoke_collector_enrollment(&originals, review.id());
            tokio::pin!(publishing, cancelling);
            // The publication takes the source writer and enters its real
            // store admission, where the probe holds it.
            tokio::time::timeout(OVERLAP_BOUND, async {
                tokio::select! {
                    biased;
                    () = probe.entered.notified() => {}
                    published = &mut publishing => {
                        panic!("the publication settled before its store admission: {published:?}")
                    }
                }
            })
            .await
            .expect("the publication enters its store admission");
            // The revocation queues on the same source writer.
            tokio::time::timeout(OVERLAP_BOUND, async {
                tokio::select! {
                    biased;
                    waiting = &mut queued => waiting.unwrap(),
                    cancelled = &mut cancelling => panic!(
                        "revocation settled while the publication held the writer: {cancelled:?}"
                    ),
                }
            })
            .await
            .expect("the revocation queues on the source writer");
            // Release the store admission and keep driving both futures.
            probe.release.notify_one();
            let published = tokio::time::timeout(OVERLAP_BOUND, async {
                tokio::select! {
                    biased;
                    published = &mut publishing => published,
                    cancelled = &mut cancelling => panic!(
                        "revocation settled before the publication: {cancelled:?}"
                    ),
                }
            })
            .await
            .expect("the publication settles once its admission is released");
            let cancelled = tokio::time::timeout(OVERLAP_BOUND, cancelling)
                .await
                .expect("the queued revocation settles after the publication");
            (published, cancelled)
        };
        // The actual serialized order: the publication linearized before the
        // revocation completed, so it returns the enrollment it committed.
        assert_eq!(published, Ok(review.id().clone()));
        if cleanup_available {
            assert_eq!(cancelled, Ok(()));
        } else {
            assert_eq!(cancelled, Err(CollectorSetupError::PendingReconciliation));
        }
        assert!(matches!(
            originals.collector_setup_not_revoked(review.id()).await,
            Err(CollectorSetupError::SetupRevoked)
        ));
        assert!(matches!(
            owner.source.original_disposition(namespace, original),
            Ok(OriginalDisposition::OperationCancelled { .. })
        ));
        assert!(
            owner
                .source
                .enrolled_service_original(namespace, original)
                .unwrap()
                .is_none()
        );
        let retried = originals.persist_collector_setup(record.clone()).await;
        assert!(
            matches!(
                retried,
                Err(CollectorSetupError::Denied | CollectorSetupError::SetupRevoked)
            ),
            "{retried:?}"
        );
        assert!(matches!(
            owner.source.original_disposition(namespace, original),
            Ok(OriginalDisposition::OperationCancelled { .. })
        ));
        let grant = owner.source.grants().resolve_lineage(
            &record.lineage,
            &record.association.logical_executor,
            None,
        );
        if cleanup_available {
            assert!(grant.is_err());
        } else {
            // The native grant remains live while its cleanup is pending
            // reconciliation: the canonical enrollment row, not native
            // cleanup, keeps the enrollment revoked.
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

/// Cancellation first: the revocation takes the source writer and holds it in
/// its own real store write, the setup tombstone's admission, here held before
/// or after the commit. An enrollment publication that overlaps it queues on
/// that writer; once the revocation has settled the publication must refuse,
/// and nothing reports the enrollment active.
#[tokio::test]
async fn revoke_first_refuses_enrollment_publication_queued_on_the_source_writer() {
    use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, OriginalDisposition};
    for (after_commit, cleanup_available) in [(false, true), (true, true), (true, false)] {
        let (_directory, selection) = crate::installed_host::tests::fixture();
        let mut owner = OperatorSetup::open(
            selection,
            OperatorOwnerStartup::fresh("collector-revoke-first", 1).unwrap(),
        )
        .unwrap();
        let probe = PauseStore::new(owner.originals.clone(), TOMBSTONE_ADMISSION, after_commit);
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
        let namespace = &record.association.ingress_namespace;
        let original = &record.association.original_work;
        // Observes the first source writer acquisition that has to wait.
        let mut queued = owner.source.observe_source_writer_wait_for_test();
        let (cancelled, published) = {
            let cancelling = owner.revoke_collector_enrollment(&originals, review.id());
            let publishing = originals.persist_collector_setup(record.clone());
            tokio::pin!(cancelling, publishing);
            // The revocation takes the source writer and enters its tombstone
            // admission, where the probe holds it.
            tokio::time::timeout(OVERLAP_BOUND, async {
                tokio::select! {
                    biased;
                    () = probe.entered.notified() => {}
                    cancelled = &mut cancelling => {
                        panic!("the revocation settled before its store admission: {cancelled:?}")
                    }
                }
            })
            .await
            .expect("the revocation enters its tombstone admission");
            // The publication queues on the same source writer.
            tokio::time::timeout(OVERLAP_BOUND, async {
                tokio::select! {
                    biased;
                    waiting = &mut queued => waiting.unwrap(),
                    published = &mut publishing => panic!(
                        "publication settled while the revocation held the writer: {published:?}"
                    ),
                }
            })
            .await
            .expect("the publication queues on the source writer");
            // Release the tombstone admission and keep driving both futures.
            probe.release.notify_one();
            let cancelled = tokio::time::timeout(OVERLAP_BOUND, async {
                tokio::select! {
                    biased;
                    cancelled = &mut cancelling => cancelled,
                    published = &mut publishing => panic!(
                        "publication settled before the revocation: {published:?}"
                    ),
                }
            })
            .await
            .expect("the revocation settles once its admission is released");
            // The settled revocation has already cancelled the original.
            assert!(matches!(
                owner.source.original_disposition(namespace, original),
                Ok(OriginalDisposition::OperationCancelled { .. })
            ));
            let published = tokio::time::timeout(OVERLAP_BOUND, publishing)
                .await
                .expect("the queued publication settles after the revocation");
            (cancelled, published)
        };
        if cleanup_available {
            assert_eq!(cancelled, Ok(()));
        } else {
            assert_eq!(cancelled, Err(CollectorSetupError::PendingReconciliation));
        }
        // The publication that queued behind the revocation refuses: it
        // neither commits nor publishes the enrollment.
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
        assert!(matches!(
            owner.source.original_disposition(namespace, original),
            Ok(OriginalDisposition::OperationCancelled { .. })
        ));
        assert!(
            owner
                .source
                .enrolled_service_original(namespace, original)
                .unwrap()
                .is_none()
        );
        let grant = owner.source.grants().resolve_lineage(
            &record.lineage,
            &record.association.logical_executor,
            None,
        );
        if cleanup_available {
            assert!(grant.is_err());
        } else {
            // The native grant remains live while its cleanup is pending
            // reconciliation: the canonical tombstone, not native cleanup,
            // refuses the queued publication.
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
        let probe = PauseStore::new(owner.originals.clone(), ENROLLMENT_ADMISSION, false);
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
