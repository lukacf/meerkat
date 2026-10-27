//! More than one source work window with actual native admissions. The selected
//! wrapper refuses terminal source writes before touching healthy SQLite. This
//! is a controlled storage failure, not native refusal or installed cold recovery.
//! The real worker starts its first round only after stop. Concurrent live-worker
//! and fresh-binder progress is outside this stopped-window control.
use super::*;
use toolkit_store::*;
use std::sync::OnceLock;
use crate::original_ingress::custody_attempt::CustodyDecision;
use crate::private_originals::custody_recovery::HandoffPhase;

const ORIGINALS: usize = 33;

struct RefuseTerminalStore {
    inner: Arc<dyn ToolkitStore>,
    terminal_calls: Mutex<BTreeMap<String, usize>>,
    reads: AtomicUsize,
}
impl RefuseTerminalStore {
    fn new(inner: Arc<dyn ToolkitStore>) -> Self {
        Self { inner, terminal_calls: Mutex::new(BTreeMap::new()), reads: AtomicUsize::new(0) }
    }
    fn calls(&self) -> (BTreeMap<String, usize>, usize) {
        (self.terminal_calls.lock().unwrap().clone(), self.reads.load(Ordering::SeqCst))
    }
}
#[async_trait::async_trait]
impl ToolkitStore for RefuseTerminalStore {
    fn capabilities(&self) -> StoreCapabilities { self.inner.capabilities() }
    async fn now_ms(&self) -> StoreResult<i64> { self.inner.now_ms().await }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> { self.inner.admit(request).await }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get(key).await
    }
    async fn list_records(&self, scope: &StoreScope, after: Option<&str>, limit: u32) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        let terminal = transition.data.body["custody_attempt"]["decision"].as_str()
            .is_some_and(|value| value == "promoted" || value == "withdrawn");
        if terminal {
            *self.terminal_calls.lock().unwrap().entry(transition.key.id.clone()).or_default() += 1;
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

// Reuse the actual fixture's same controller, destination and native session.
// Only the new source event/association changes; no new authority is invented.
async fn another_original(fixture: &Fixture, index: usize) -> PrivateOriginalSubmit {
    let label = format!("stopped-original-{index}");
    let mut event = private_event(&label);
    event.content_sha256 = digest(PROMPT.as_bytes());
    let authentication = event.original_authentication.clone();
    let reference = fixture.source.retain_event(event).await.unwrap();
    let current = fixture.association.candidate();
    let mut candidate = claims(&label, &current.controller_grant_lineage[0]).candidate().clone();
    candidate.authority_basis = current.authority_basis.clone();
    candidate.target = current.target.clone();
    candidate.controller_grant_lineage = current.controller_grant_lineage.clone();
    candidate.controller_model = current.controller_model.clone();
    candidate.controller_ceiling = current.controller_ceiling.clone();
    let association = InputAuthorityAssociation::new(candidate).unwrap();
    let original = fixture.owner.bind_controller_only_async(
        association.clone(), source_custody(&association), &principal("user"), &principal("frontend"),
        |final_association| {
            let source = fixture.source.clone(); let reference = reference.clone();
            async move { source.persist_binding(&reference, &final_association).await }
        },
    ).await.unwrap();
    fixture.source.hold_binding(reference.clone(), original).await.unwrap();
    PrivateOriginalSubmit { original: reference, content: PROMPT.into(), transport_authentication: authentication }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_owner_visits_more_than_one_window_and_caches_failed_terminal_writes() {
    let capture = Arc::new(OnceLock::<Arc<RefuseTerminalStore>>::new());
    let selected = capture.clone();
    let fixture = Fixture::with_source_store(DeliveryMode::Normal, false,
        Some(Box::new(move |inner, _fault| {
            let store = Arc::new(RefuseTerminalStore::new(inner));
            assert!(selected.set(store.clone()).is_ok()); store
        }))).await;
    let store = capture.get().unwrap().clone();
    let mut first_round = Some(fixture.source.hold_first_custody_round_for_test());
    let mut native_shutdown = Box::pin(fixture.shutdown());
    let mut native_cleanup = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 8, async {
        let mut submissions = vec![fixture.submission.clone()];
        for index in 1..ORIGINALS { submissions.push(another_original(&fixture, index).await); }
        assert!(!fixture.owner.has_installed_pair_recovery().unwrap());
        fixture.source.start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer()).unwrap();
        let mut retained = Vec::new();
        for (index, submission) in submissions.iter().enumerate() {
            let result = fixture.submit_with(submission.clone()).await;
            let Ok(PrivateOriginalSubmission::Admitted {
                input_id, deduplicated: false, turn_ticket: Some(ticket),
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
            }) = &result else {
                panic!("actual native acceptance required at original {index}; accepted={}; terminal_calls={}: {}",
                    fixture.fault.accepted.load(Ordering::SeqCst), store.calls().0.len(), submission_context(&result));
            };
            fixture.finish(ticket).await.unwrap();
            let held = fixture.source.retained.lock().unwrap().iter()
                .find(|held| held.reference == submission.original).cloned().unwrap();
            let pending = held.first_custody.lock().unwrap().clone().unwrap();
            assert!(pending.handoff.enrolled() && pending.first_candidate);
            assert!(pending.handoff.phase() == HandoffPhase::Accepted);
            assert_eq!(input_id, &pending.input.id().to_string());
            let receipt = pending.native_admission.lock().unwrap().clone().unwrap();
            assert_eq!(receipt.input_id, *pending.input.id());
            assert_eq!(receipt.session_id, fixture.session);
            assert!(!receipt.deduplicated);
            assert!(matches!(&*pending.pending.lock().unwrap(),
                SourcePublication::Settled { source_acknowledged: true, result: Ok(()) }));
            let key = fixture.source.key(&submission.original).unwrap();
            let row = store.inner.get(&key).await.unwrap().unwrap();
            let attempt = fixture.source.decode_record(&row, &submission.original).unwrap().custody_attempt.unwrap();
            assert!(attempt.decision() == CustodyDecision::Pending);
            assert!(attempt.matches_input(&pending.input).unwrap());
            retained.push((submission.original.clone(), key, row, pending));
        }
        assert_eq!(retained.len(), ORIGINALS);
        assert_eq!(fixture.native_inputs().await, Ok((ORIGINALS, ORIGINALS)));
        assert_eq!(fixture.fault.accepted.load(Ordering::SeqCst), ORIGINALS);
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), ORIGINALS);
        assert!(store.calls().0.is_empty(), "no recovery terminal attempt may precede stop");
        fixture.source.stop_custody_recovery();
        first_round.take().unwrap().send(()).unwrap();
        native_cleanup = Some(native_shutdown.as_mut().await);
        native_cleanup.as_ref().unwrap().as_ref().unwrap();
        assert!(matches!(fixture.source.join_custody_recovery().await, Err(PrivateOriginalError::Unavailable)));
        let final_calls = store.calls();
        assert_eq!(final_calls.0.len(), ORIGINALS, "every retained entry must receive a terminal attempt");
        for (reference, key, before, pending) in &retained {
            assert!(final_calls.0.get(&key.id).is_some_and(|calls| *calls > 0));
            assert!(pending.handoff.closed_failure(), "a failed entry must leave the stopped work window");
            assert!(pending.handoff.phase() == HandoffPhase::Accepted, "failed close is not a terminal source decision");
            assert_eq!(pending.handoff.observations.load(Ordering::SeqCst), 0);
            let row = store.inner.get(key).await.unwrap().unwrap();
            assert!(row == *before, "failed source writes must preserve the exact canonical row");
            assert!(fixture.source.decode_record(&row, reference).unwrap().custody_attempt.unwrap()
                .decision() == CustodyDecision::Pending);
            let receipt = pending.native_admission.lock().unwrap().clone().unwrap();
            assert_eq!(receipt.input_id, *pending.input.id());
            assert_eq!(receipt.session_id, fixture.session);
            assert!(matches!(&*pending.pending.lock().unwrap(),
                SourcePublication::Settled { source_acknowledged: true, result: Ok(()) }));
        }
        for _ in 0..2 {
            assert!(matches!(fixture.source.join_custody_recovery().await, Err(PrivateOriginalError::Unavailable)));
            assert_eq!(store.calls(), final_calls, "rejoining cached failure must perform no new read or CAS");
        }
        assert_eq!(fixture.fault.accepted.load(Ordering::SeqCst), ORIGINALS);
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), ORIGINALS);
    })).catch_unwind().await;
    fixture.source.stop_custody_recovery();
    drop(first_round.take());
    let native = match native_cleanup { Some(result) => result, None => native_shutdown.as_mut().await };
    let source = fixture.source.join_custody_recovery().await;
    match checked {
        Ok(Ok(())) => {
            assert!(native.is_ok(), "actual native cleanup failed: {native:?}");
            assert!(matches!(source, Err(PrivateOriginalError::Unavailable)));
        }
        Ok(Err(_)) => panic!("stopped traversal timed out; native={native:?}; source={source:?}"),
        Err(payload) => {
            if native.is_err() { eprintln!("native cleanup failed: {native:?}"); }
            std::panic::resume_unwind(payload);
        }
    }
}
