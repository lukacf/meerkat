//! An actual native binder queues behind the same source owner's entered
//! recovery CAS. No worker gate hides this live overlap.
use super::*;
use toolkit_store::*;
use std::sync::{OnceLock, atomic::AtomicBool};
use crate::original_ingress::custody_attempt::CustodyDecision;
use crate::private_originals::custody_recovery::HandoffPhase;

struct HeldRecoveryWrite {
    inner: Arc<dyn ToolkitStore>, hold: AtomicBool,
    entered: tokio::sync::Notify, release: tokio::sync::Semaphore,
    entered_count: AtomicUsize, returned_count: AtomicUsize,
}
impl HeldRecoveryWrite {
    fn new(inner: Arc<dyn ToolkitStore>) -> Self {
        Self { inner, hold: AtomicBool::new(true), entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0), entered_count: AtomicUsize::new(0),
            returned_count: AtomicUsize::new(0) }
    }
}
#[async_trait::async_trait]
impl ToolkitStore for HeldRecoveryWrite {
    fn capabilities(&self) -> StoreCapabilities { self.inner.capabilities() }
    async fn now_ms(&self) -> StoreResult<i64> { self.inner.now_ms().await }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> { self.inner.admit(request).await }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> { self.inner.get(key).await }
    async fn list_records(&self, scope: &StoreScope, after: Option<&str>, limit: u32) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        if transition.data.body["custody_attempt"]["decision"] == "promoted"
            && self.hold.swap(false, Ordering::SeqCst) {
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

// Select B before starting recovery so the test isolates the actual binder's
// first-attempt write, with the same real controller/session as A.
async fn second_original(fixture: &Fixture) -> PrivateOriginalSubmit {
    let label = "writer-wait-original";
    let mut event = private_event(label);
    event.content_sha256 = digest(PROMPT.as_bytes());
    let authentication = event.original_authentication.clone();
    let reference = fixture.source.retain_event(event).await.unwrap();
    let current = fixture.association.candidate();
    let mut candidate = claims(label, &current.controller_grant_lineage[0]).candidate().clone();
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

#[derive(Clone, Copy)]
enum WaitingCaller { Live, Lost, SourceClosed }

async fn exercise(mode: WaitingCaller) {
    let capture = Arc::new(OnceLock::<Arc<HeldRecoveryWrite>>::new());
    let selected = capture.clone();
    let fixture = Fixture::with_source_store(DeliveryMode::Normal, false,
        Some(Box::new(move |inner, _fault| {
            let store = Arc::new(HeldRecoveryWrite::new(inner));
            assert!(selected.set(store.clone()).is_ok()); store
        }))).await;
    let store = capture.get().unwrap().clone();
    let mut native_shutdown = Box::pin(fixture.shutdown());
    let mut native_cleanup = None;
    // The public observer is retained outside the body timeout. Dropping it
    // after entry does not drop Meta's actual tracked binder/write operation.
    let mut caller = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 4, async {
        let second = second_original(&fixture).await;
        let b = fixture.source.retained.lock().unwrap().iter()
            .find(|held| held.reference == second.original).cloned().unwrap();
        let b_key = fixture.source.key(&second.original).unwrap();
        let before_b = store.inner.get(&b_key).await.unwrap().unwrap();
        assert!(fixture.source.decode_record(&before_b, &second.original).unwrap().custody_attempt.is_none());
        fixture.source.start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer()).unwrap();
        let first = fixture.submit().await;
        let Ok(PrivateOriginalSubmission::Admitted { deduplicated: false, turn_ticket: Some(ticket),
            source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, .. }) = &first
            else { panic!("A requires actual native admission: {}", submission_context(&first)); };
        fixture.finish(ticket).await.unwrap();
        store.entered.notified().await;
        assert_eq!(store.entered_count.load(Ordering::SeqCst), 1);
        assert_eq!(store.returned_count.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.native_inputs().await, Ok((1, 1)));
        let a = fixture.source.retained.lock().unwrap().iter()
            .find(|held| held.reference == fixture.submission.original).cloned().unwrap();
        let a_pending = a.first_custody.lock().unwrap().clone().unwrap();
        assert!(a_pending.handoff.phase() == HandoffPhase::Accepted);
        let a_row = store.inner.get(&fixture.source.key(&fixture.submission.original).unwrap()).await.unwrap().unwrap();
        assert!(fixture.source.decode_record(&a_row, &fixture.submission.original).unwrap()
            .custody_attempt.unwrap().decision() == CustodyDecision::Pending);

        // There is exactly one recovery task, stopped inside the CAS above.
        // No other async source mutation runs; this signal can only be B's
        // real binder polling the same mutex to Pending.
        let waiting = fixture.owner.observe_source_writer_wait_for_test();
        caller = Some(Box::pin({
            let source = fixture.source.clone(); let submission = second.clone();
            async move { source.submit_authenticated_original(submission).await }
        }));
        tokio::select! {
            queued = waiting => queued.unwrap(),
            result = caller.as_mut().unwrap().as_mut() => {
                panic!("B returned before the held writer released: {}", submission_context(&result));
            }
        }
        assert!(matches!(futures::poll!(caller.as_mut().unwrap().as_mut()), std::task::Poll::Pending));
        assert!(store.inner.get(&b_key).await.unwrap().unwrap() == before_b);
        assert!(b.first_custody.lock().unwrap().is_none());
        assert_eq!(fixture.fault.accepted.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 1);
        match mode {
            WaitingCaller::Live => (),
            WaitingCaller::Lost => { drop(caller.take()); },
            WaitingCaller::SourceClosed => fixture.source.stop_custody_recovery(),
        }
        store.release.add_permits(1);
        if !matches!(mode, WaitingCaller::Lost) {
            let result = caller.as_mut().unwrap().as_mut().await;
            drop(caller.take());
            match mode {
                WaitingCaller::Live => {
                    let Ok(PrivateOriginalSubmission::Admitted { deduplicated: false,
                        turn_ticket: Some(ticket), source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, .. }) = &result
                        else { panic!("B must admit after the writer releases: {}", submission_context(&result)); };
                    fixture.finish(ticket).await.unwrap();
                }
                WaitingCaller::SourceClosed => {
                    assert!(matches!(result, Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)),
                        "a close that won during the wait must refuse enrollment");
                }
                WaitingCaller::Lost => unreachable!(),
            }
        }
        if !matches!(mode, WaitingCaller::SourceClosed) {
            // The lost public observer supplies no ticket. Inspect the same
            // retained native receipt and actual terminal input, with no replay.
            loop {
                let pending = b.first_custody.lock().unwrap().clone();
                if let Some(pending) = pending {
                    let receipt = pending.native_admission.lock().unwrap().clone();
                    if let Some(receipt) = receipt {
                        assert_eq!(receipt.input_id, *pending.input.id());
                        assert_eq!(receipt.session_id, fixture.session);
                        assert!(!receipt.deduplicated);
                        if let Some(native) = fixture.machine.input_state(&fixture.session, pending.input.id()).await.unwrap() {
                            if native.seed.phase == meerkat_runtime::input_state::InputLifecycleState::Consumed {
                                meerkat_runtime::input_state::verify_exact_replay(&native.state, &pending.input).unwrap();
                                break;
                            }
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            assert_eq!(fixture.native_inputs().await, Ok((2, 2)));
            assert_eq!(fixture.fault.accepted.load(Ordering::SeqCst), 2);
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 2);
        } else {
            assert!(b.first_custody.lock().unwrap().is_none());
            assert_eq!(fixture.native_inputs().await, Ok((1, 1)));
            assert_eq!(fixture.fault.accepted.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.provider.0.load(Ordering::SeqCst), 1);
            let row = store.inner.get(&b_key).await.unwrap().unwrap();
            assert!(fixture.source.decode_record(&row, &second.original).unwrap().custody_attempt.unwrap()
                .decision() == CustodyDecision::Pending,
                "a pre-Candidate write is not a terminal custody or nonadmission claim");
        }
        fixture.source.stop_custody_recovery();
        native_cleanup = Some(native_shutdown.as_mut().await);
        native_cleanup.as_ref().unwrap().as_ref().unwrap();
        fixture.source.join_custody_recovery().await.unwrap();
        assert_eq!(store.returned_count.load(Ordering::SeqCst), 1);
        assert!(a_pending.handoff.phase() == HandoffPhase::Done);
        let final_b = store.inner.get(&b_key).await.unwrap().unwrap();
        if !matches!(mode, WaitingCaller::SourceClosed) {
            let pending = b.first_custody.lock().unwrap().clone().unwrap();
            let attempt = fixture.source.decode_record(&final_b, &second.original).unwrap().custody_attempt.unwrap();
            assert!(attempt.decision() == CustodyDecision::Promoted && attempt.matches_input(&pending.input).unwrap());
        }
        fixture.source.join_custody_recovery().await.unwrap();
        assert!(store.inner.get(&b_key).await.unwrap().unwrap() == final_b);
    })).catch_unwind().await;
    // Release the actual entered write before joining any retained operation.
    // The public caller may be abandoned, while native still owns its binder.
    fixture.source.stop_custody_recovery();
    store.release.add_permits(1);
    drop(caller.take());
    let native = match native_cleanup { Some(result) => result, None => native_shutdown.as_mut().await };
    let source = fixture.source.join_custody_recovery().await;
    match checked {
        Ok(Ok(())) => assert!(native.is_ok() && source.is_ok(), "cleanup: native={native:?}; source={source:?}"),
        Ok(Err(_)) => panic!("writer contention control timed out; native={native:?}; source={source:?}"),
        Err(payload) => {
            if native.is_err() || source.is_err() { eprintln!("cleanup: native={native:?}; source={source:?}"); }
            std::panic::resume_unwind(payload);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_binder_waits_for_recovery_write_then_admits() { exercise(WaitingCaller::Live).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_caller_keeps_actual_binder_wait_owned_until_admission() { exercise(WaitingCaller::Lost).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_close_during_writer_wait_refuses_new_candidate_after_release() { exercise(WaitingCaller::SourceClosed).await; }
