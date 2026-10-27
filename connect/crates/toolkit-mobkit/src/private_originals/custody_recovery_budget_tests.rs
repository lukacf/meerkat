//! Actual source task/native input with controlled prepaid canonical budgets.
//! The setup below does not claim seven/eight historical native observations.
//! It exercises the final allowed step and zero-new-step rule against real
//! admission, SQLite terminal writes, lost replies and a cancelled join observer.
use super::*;
use toolkit_store::*;
use std::sync::{OnceLock, atomic::AtomicBool};
use crate::original_ingress::custody_attempt::{CustodyChange, CustodyDecision};
use crate::private_originals::custody_recovery::HandoffPhase;

struct BudgetStore {
    inner: Arc<dyn ToolkitStore>, fault: Arc<DeliveryFault>,
    fail_read: AtomicBool,
    hold_read: AtomicBool, entered: tokio::sync::Notify, release: tokio::sync::Semaphore,
    hold_terminal: AtomicBool, terminal_entered: tokio::sync::Notify,
    terminal_release: tokio::sync::Semaphore, terminal_commits: AtomicUsize,
    terminal_returned: AtomicUsize,
}
impl BudgetStore {
    fn new(inner: Arc<dyn ToolkitStore>, fault: Arc<DeliveryFault>) -> Self {
        Self { inner, fault, fail_read: AtomicBool::new(false), hold_read: AtomicBool::new(true), entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0), hold_terminal: AtomicBool::new(true),
            terminal_entered: tokio::sync::Notify::new(), terminal_release: tokio::sync::Semaphore::new(0),
            terminal_commits: AtomicUsize::new(0), terminal_returned: AtomicUsize::new(0) }
    }
}
#[async_trait::async_trait]
impl ToolkitStore for BudgetStore {
    fn capabilities(&self) -> StoreCapabilities { self.inner.capabilities() }
    async fn now_ms(&self) -> StoreResult<i64> { self.inner.now_ms().await }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> { self.inner.admit(request).await }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        if self.fault.accepted.load(Ordering::SeqCst) > 0 && self.hold_read.swap(false, Ordering::SeqCst) {
            // Gate only after the actual bridge accepted. Foreground binding
            // has finished its source writes; the recovery read is now entered.
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        if self.fail_read.load(Ordering::SeqCst) { return Err(StoreError::OwnerUnavailable); }
        self.inner.get(key).await
    }
    async fn list_records(&self, scope: &StoreScope, after: Option<&str>, limit: u32) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        let terminal = transition.data.body["custody_attempt"]["decision"].as_str()
            .is_some_and(|value| value == "promoted" || value == "withdrawn");
        if terminal && self.hold_terminal.swap(false, Ordering::SeqCst) {
            self.inner.compare_and_swap(transition).await?;
            self.terminal_commits.fetch_add(1, Ordering::SeqCst);
            self.terminal_entered.notify_one();
            self.terminal_release.acquire().await.unwrap().forget();
            self.terminal_returned.fetch_add(1, Ordering::SeqCst);
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

async fn exercise(final_step: bool) {
    let capture = Arc::new(OnceLock::<Arc<BudgetStore>>::new());
    let slot = capture.clone();
    let fixture = Fixture::with_source_store(DeliveryMode::LoseAcceptedAnswer, false,
        Some(Box::new(move |inner, fault| {
            let selected = Arc::new(BudgetStore::new(inner, fault));
            assert!(slot.set(selected.clone()).is_ok()); selected
        }))).await;
    let store = capture.get().unwrap().clone();
    // Keep the same native shutdown future if the body observer is cancelled;
    // a completed result is checked again without a second shutdown call.
    let mut native_shutdown = Box::pin(fixture.shutdown());
    let mut native_cleanup = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 3, async {
        assert!(!fixture.source.owner.has_installed_pair_recovery().unwrap(),
            "a generic selected adapter cannot claim installed cold eligibility");
        fixture.source.start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer()).unwrap();
        let result = fixture.submit().await;
        assert!(matches!(&result, Ok(PrivateOriginalSubmission::AdmissionUnresolved)),
            "actual lost bridge reply must remain unresolved: {}", submission_context(&result));
        store.entered.notified().await;
        let held = fixture.source.retained.lock().unwrap().iter()
            .find(|held| held.reference == fixture.submission.original).cloned().unwrap();
        let pending = held.first_custody.lock().unwrap().clone().unwrap();
        assert!(pending.handoff.enrolled());
        assert!(pending.native_admission.lock().unwrap().is_none());
        assert!(pending.handoff.phase() == HandoffPhase::Resolving);
        let claims = fixture.association.candidate();
        assert!(matches!(fixture.owner.retained_custody(&claims.ingress_namespace, &claims.original_work),
            Ok(meerkat_runtime::meerkat_machine::RetainedCustody::AdmissionPending)));
        let key = fixture.source.key(&fixture.submission.original).unwrap();
        let original = store.inner.get(&key).await.unwrap().unwrap();
        let first = fixture.source.decode_record(&original, &fixture.submission.original).unwrap().custody_attempt.unwrap();
        // Controlled prior-step setup uses the actual codec/CAS and writer,
        // while the one real owner is paused before its first native read.
        let spent = if final_step { 7 } else { 8 };
        {
            let _writer = fixture.source.owner.source_writer().await;
            for _ in 0..spent {
                for change in [CustodyChange::ChargeResolution, CustodyChange::ResolutionSettled] {
                    let row = store.inner.get(&key).await.unwrap().unwrap();
                    let current = fixture.source.decode_record(&row, &fixture.submission.original).unwrap().custody_attempt.unwrap();
                    let next = current.changed(change).unwrap();
                    store.inner.compare_and_swap(fixture.source.prepare_custody_transition(
                        &row, &fixture.submission.original, next).unwrap()).await.unwrap();
                }
            }
        }
        assert_eq!(pending.handoff.observations.load(Ordering::SeqCst), 0);
        if !final_step { store.fail_read.store(true, Ordering::SeqCst); }
        store.release.add_permits(1);
        if !final_step {
            // The actual owner observes an unreadable canonical store. Reader
            // calls neither spend its budget nor turn that failure into absence.
            tokio::time::timeout(WAIT, async {
                loop {
                    if !pending.handoff.canonical_readable_for_test() { break; }
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
            assert!(matches!(fixture.owner.retained_custody(&claims.ingress_namespace, &claims.original_work),
                Err(meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable)));
            assert_eq!(pending.handoff.observations.load(Ordering::SeqCst), 0);
            store.fail_read.store(false, Ordering::SeqCst);
            fixture.source.custody_recovery.changed();
        }
        store.terminal_entered.notified().await;
        assert_eq!(store.terminal_commits.load(Ordering::SeqCst), 1);
        assert_eq!(store.terminal_returned.load(Ordering::SeqCst), 0);
        let committed = store.inner.get(&key).await.unwrap().unwrap();
        let decision = fixture.source.decode_record(&committed, &fixture.submission.original).unwrap().custody_attempt.unwrap();
        assert!(decision.same_attempt(&first));
        assert!(decision.decision() == if final_step { CustodyDecision::Promoted } else { CustodyDecision::Withdrawn });
        assert_eq!(committed.data.body["custody_attempt"]["spent_steps"], 8);
        assert_eq!(pending.handoff.observations.load(Ordering::SeqCst), usize::from(final_step));
        // Even the zero-budget withdrawal does not assert native nonadmission.
        let native = fixture.machine.input_state(&fixture.session, pending.input.id()).await.unwrap().unwrap();
        meerkat_runtime::input_state::verify_exact_replay(&native.state, &pending.input).unwrap();
        assert_eq!(fixture.native_inputs().await, Ok((1, 1)));
        fixture.source.stop_custody_recovery();
        assert!(!matches!(fixture.owner.retained_custody(&claims.ingress_namespace, &claims.original_work),
            Ok(meerkat_runtime::meerkat_machine::RetainedCustody::AdmissionPending)));
        native_cleanup = Some(native_shutdown.as_mut().await);
        native_cleanup.as_ref().unwrap().as_ref().unwrap();
        let mut joining = Box::pin(fixture.source.join_custody_recovery());
        assert!(matches!(futures::poll!(joining.as_mut()), std::task::Poll::Pending));
        drop(joining);
        assert_eq!(store.terminal_returned.load(Ordering::SeqCst), 0);
        store.terminal_release.add_permits(1);
        fixture.source.join_custody_recovery().await.unwrap();
        assert_eq!(store.terminal_returned.load(Ordering::SeqCst), 1);
        assert!(pending.handoff.phase() == HandoffPhase::Done);
        assert!(matches!(fixture.owner.retained_custody(&claims.ingress_namespace, &claims.original_work),
            Ok(meerkat_runtime::meerkat_machine::RetainedCustody::NotRetained)),
            "withdrawal or unpublished close is never pending, even when the exact native input exists");
        assert!(store.inner.get(&key).await.unwrap().unwrap() == committed,
            "lost terminal reply is resolved by exact readback, without another write");
        assert_eq!(pending.handoff.observations.load(Ordering::SeqCst), usize::from(final_step),
            "lost terminal reply cannot cause another native observation");
        assert_eq!(fixture.fault.accepted.load(Ordering::SeqCst), 1);
        fixture.source.join_custody_recovery().await.unwrap();
    })).catch_unwind().await;
    fixture.source.stop_custody_recovery();
    store.fail_read.store(false, Ordering::SeqCst);
    store.release.add_permits(1); store.terminal_release.add_permits(1);
    let native = match native_cleanup {
        Some(completed) => completed,
        None => native_shutdown.as_mut().await,
    };
    let source = fixture.source.join_custody_recovery().await;
    assert!(native.is_ok() && source.is_ok(), "actual cleanup: native={native:?}, source={source:?}");
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("bounded recovery control timed out"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn final_prepaid_step_promotes_actual_native_input_and_joins_lost_terminal_reply() { exercise(true).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhausted_budget_withdraws_source_without_new_native_read_or_nonadmission_claim() { exercise(false).await; }
