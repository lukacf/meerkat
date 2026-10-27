//! The installed source census wakes the actual retained native delivery owner.
//! Only the source's first page read is paused. Native admission, restoration,
//! continuation reservation and delivery remain the existing production path.
use super::*;
use crate::original_ingress::custody_attempt::{CustodyAttempt, CustodyDecision};
use meerkat_runtime::{LogicalRuntimeId, input_state::{InputLifecycleState, verify_exact_replay}};
use std::sync::{OnceLock, atomic::AtomicUsize};
use toolkit_store::*;

struct CensusStore {
    inner: Arc<dyn ToolkitStore>,
    scope: StoreScope,
    armed: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    started: AtomicUsize,
    completed: AtomicUsize,
    cancelled: AtomicUsize,
}
impl CensusStore {
    fn new(inner: Arc<dyn ToolkitStore>, scope: StoreScope) -> Self {
        Self { inner, scope, armed: AtomicBool::new(false),
            entered: tokio::sync::Notify::new(), release: tokio::sync::Semaphore::new(0),
            started: AtomicUsize::new(0), completed: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0) }
    }
    fn arm(&self) {
        assert!(!self.armed.swap(true, Ordering::SeqCst));
    }
}
struct EnteredRead<'a> { store: &'a CensusStore, complete: bool }
impl Drop for EnteredRead<'_> {
    fn drop(&mut self) {
        if !self.complete { self.store.cancelled.fetch_add(1, Ordering::SeqCst); }
    }
}
#[async_trait::async_trait]
impl ToolkitStore for CensusStore {
    fn capabilities(&self) -> StoreCapabilities { self.inner.capabilities() }
    async fn now_ms(&self) -> StoreResult<i64> { self.inner.now_ms().await }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> {
        self.inner.admit(request).await
    }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> {
        self.inner.get(key).await
    }
    async fn list_records(&self, scope: &StoreScope, after: Option<&str>, limit: u32)
        -> StoreResult<Vec<StoredRecord>>
    {
        if scope == &self.scope && self.armed.swap(false, Ordering::SeqCst) {
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


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_census_wakes_existing_completion_after_native_source_restore() {
    let (fixture, store, retained, adapter) = ContinuationFixture::for_durable_continuation().await;
    let mut active = Some(Arc::new(fixture));
    let mut native_close = active.as_ref().unwrap().retained_close_for_test();
    let mut runtime_store = Some(store);
    let mut retained = Some(retained);
    let mut continuation = Some(adapter);
    let selected_gate = Arc::new(OnceLock::<Arc<CensusStore>>::new());
    let checked = AssertUnwindSafe(tokio::time::timeout(EVENT_GUARD * 6, Box::pin(async {
        let (obligation, receipt, association, source_record, original_input, native_record, session) = {
            let fixture = active.as_ref().unwrap();
            let adapter = continuation.as_ref().unwrap();
            let announcer = fixture.continuation_native().continuation_source_announcer();
            assert!(announcer.is_some(), "this actual fixture composes native delivery");
            fixture.source.start_custody_recovery(announcer).unwrap();
            let admitted = fixture.submit().await;
            assert!(matches!(&admitted, Ok(crate::private_originals::PrivateOriginalSubmission::Admitted {
                deduplicated: false,
                source_settlement: crate::private_originals::PrivateSourceSettlement::FreshAcceptancePromoted,
                ..
            })), "the original must actually admit and publish source custody");
            fixture.finish(&admitted).await.unwrap();
            let context = fixture.probe.as_ref().unwrap().captured();
            tokio::time::timeout(EVENT_GUARD, fixture.machine.wait_run_settled(
                &fixture.session, context.retained_work().unwrap().run_id())).await.unwrap();
            let association = fixture.binding().await.unwrap().unwrap();
            // Live Task custody retains its real issuer; the exact cold rebind preserves it.
            assert_eq!(fixture.cold_source_custody(&association), Some((true, false)));
            // Join the actual first-attempt publication before intentionally
            // losing only its volatile source handle for the pending delivery.
            fixture.source.stop_custody_recovery();
            fixture.source.join_custody_recovery().await.unwrap();
            let rows = runtime_store.as_ref().unwrap().load_input_states_strict(
                &LogicalRuntimeId::for_session(&fixture.session)).await.unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].seed.phase, InputLifecycleState::Consumed);
            assert!(rows[0].state.persisted_input.is_none(), "completed native payload was retired");
            let source = fixture.cold_source_record().await;
            let original_input: meerkat_runtime::Input = serde_json::from_value(
                source.data.body["bound_service_input"].clone()).unwrap();
            assert!(original_input.header().ingress_context.is_none(), "canonical bytes retain no process owner");
            verify_exact_replay(&rows[0].state, &original_input).unwrap();
            let state = fixture.continuation_native().agent_mob_mcp_state().unwrap();
            let service = state.continuation_service().unwrap();
            let obligation = terminal_result(adapter, fixture, &context).await;
            let owner = InstalledNativeContinuation::owner(adapter.binding(&obligation).unwrap());
            let delivery = adapter.delivery(&obligation).unwrap();
            let job = RetainedJobRecord::from_owner(adapter.as_ref(), adapter.producer(), obligation.operation().as_str())
                .await.unwrap().unwrap();
            let attempt: CustodyAttempt = serde_json::from_value(source.data.body["custody_attempt"].clone()).unwrap();
            assert_eq!(source.data.version, 4);
            assert!(attempt.decision() == CustodyDecision::Promoted);
            attempt.validate_binding(&association).unwrap();
            assert!(attempt.matches_input(&original_input).unwrap());
            fixture.forget_source_custody(&association);
            let receipt = service.submit_retained_completion(&owner, delivery.clone(), &job,
                u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap()).await.unwrap();
            assert!(matches!(service.continuation_status(&owner, &delivery.key).await.unwrap(),
                ContinuationStatus::Pending { receipt: ref observed, admitted: None, .. } if observed == &receipt));
            (obligation, receipt, association, source, original_input,
                serde_json::to_value(&rows[0]).unwrap(), fixture.session.clone())
        };
        let weak_source = Arc::downgrade(&active.as_ref().unwrap().source);
        let weak_machine = Arc::downgrade(&active.as_ref().unwrap().machine);
        let weak_service = Arc::downgrade(&active.as_ref().unwrap().service);
        let weak_lifetime = Arc::downgrade(active.as_ref().unwrap().lifetime());
        let weak_store = Arc::downgrade(runtime_store.as_ref().unwrap());
        let weak_adapter = Arc::downgrade(continuation.as_ref().unwrap());
        native_close.join().await.unwrap();
        active.as_ref().unwrap().source.join_custody_recovery().await.unwrap();
        drop(continuation.take());
        drop(active.take());
        drop(runtime_store.take());
        assert!(weak_adapter.upgrade().is_none());
        assert!(weak_source.upgrade().is_none());
        assert!(weak_machine.upgrade().is_none());
        assert!(weak_service.upgrade().is_none());
        assert!(weak_lifetime.upgrade().is_none());
        assert!(weak_store.upgrade().is_none());

        let scope = StoreScope { realm: source_record.key.realm.clone(), owner: source_record.key.owner.clone() };
        let capture = selected_gate.clone();
        let (fixture, store, adapter) = ContinuationFixture::reopen_durable_continuation_with_source(
            retained.take().unwrap(), Box::new(move |inner| {
                let gate = Arc::new(CensusStore::new(inner, scope));
                assert!(capture.set(gate.clone()).is_ok());
                gate
            })).await;
        active = Some(Arc::new(fixture));
        native_close = active.as_ref().unwrap().retained_close_for_test();
        runtime_store = Some(store);
        continuation = Some(adapter);
        let fixture = active.as_ref().unwrap();
        let adapter = continuation.as_ref().unwrap();
        let gate = selected_gate.get().unwrap();
        assert_eq!(fixture.session, session);
        assert_eq!(fixture.model_requests(), 0, "reopen must not replay the original");
        assert!(fixture.cold_source_custody(&association).is_none());
        assert_eq!(fixture.cold_source_record().await, source_record);
        gate.arm();
        let announcer = fixture.continuation_native().continuation_source_announcer();
        assert!(announcer.is_some());
        fixture.source.start_custody_recovery(announcer).unwrap();
        gate.entered.notified().await;
        assert_eq!(gate.started.load(Ordering::SeqCst), 1);
        assert_eq!(gate.completed.load(Ordering::SeqCst), 0);
        assert_eq!(gate.cancelled.load(Ordering::SeqCst), 0);

        let address = LogicalRuntimeId::for_session(&session);
        let blocked = tokio::time::timeout(EVENT_GUARD, async {
            loop {
                if let Some(pass) = fixture.continuation_native().runtime_delivery_pass()
                    && pass.generation > 0 && pass.blocked_sessions.contains(&session)
                { break pass; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert_eq!(blocked.applied, 0);
        assert_eq!(blocked.refused, 0);
        assert!(!blocked.failures.is_empty());
        let reserved = runtime_store.as_ref().unwrap()
            .load_continuation_admission(&address, &receipt.delivery_id).await.unwrap();
        let reserved_input = match &reserved {
            Some(meerkat_runtime::ContinuationAdmission::Reserved { session_id, input_id }) => {
                assert_eq!(session_id, &session);
                assert_ne!(input_id, original_input.id());
                input_id.clone()
            },
            other => panic!("actual blocked delivery must retain its reservation: {other:?}"),
        };
        let rows = runtime_store.as_ref().unwrap().load_input_states_strict(&address).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(serde_json::to_value(&rows[0]).unwrap(), native_record);
        verify_exact_replay(&rows[0].state, &original_input).unwrap();

        // This exact helper is shared with the separate installed cold-native
        // control. It binds the saved source only: no send, Candidate or hint.
        let original = fixture.bind_existing_source_for_recovery_test(&association).await;
        // Binding retains Task issuer Some; recovery changes admission only.
        assert_eq!(fixture.cold_source_custody(&association), Some((false, false)));
        fixture.source.restore_promoted_custody(&source_record.key, &original).await.unwrap();
        assert_eq!(fixture.cold_source_custody(&association), Some((true, false)));
        assert_eq!(fixture.cold_source_record().await, source_record);
        assert_eq!(fixture.model_requests(), 0);
        // Observe only the durable reservation. Neither status resolution nor
        // adapter reconciliation may supply the wake this control requires.
        let held_observation = tokio::time::timeout(Duration::from_millis(150), async {
            loop {
                let current = runtime_store.as_ref().unwrap()
                    .load_continuation_admission(&address, &receipt.delivery_id).await.unwrap();
                assert_eq!(current, reserved, "delivery advanced before the held census could announce");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await;
        assert!(held_observation.is_err());
        assert_eq!(gate.completed.load(Ordering::SeqCst), 0);
        gate.release.add_permits(1);
        let (applied_session, input) = tokio::time::timeout(EVENT_GUARD, async {
            loop {
                if let Some(meerkat_runtime::ContinuationAdmission::Applied { session_id, input_id }) =
                    runtime_store.as_ref().unwrap().load_continuation_admission(&address, &receipt.delivery_id).await.unwrap()
                { break (session_id, input_id); }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert_eq!(gate.completed.load(Ordering::SeqCst), 1);
        assert_eq!(gate.cancelled.load(Ordering::SeqCst), 0);
        assert_eq!(applied_session, session);
        assert_eq!(input, reserved_input, "terminal source census wakes the same existing reservation");
        let pass = fixture.continuation_native().runtime_delivery_pass().unwrap();
        assert!(pass.generation > blocked.generation);
        let rows = runtime_store.as_ref().unwrap().load_input_states_strict(&address).await.unwrap();
        assert_eq!(rows.len(), 2, "only the existing committed completion may add an input");
        let original_row = rows.iter().find(|row| row.state.input_id == *original_input.id()).unwrap();
        assert_eq!(serde_json::to_value(original_row).unwrap(), native_record,
            "the wake preserves the complete original native terminal record");
        assert_eq!(original_row.seed.phase, InputLifecycleState::Consumed);
        verify_exact_replay(&original_row.state, &original_input).unwrap();
        assert_eq!(original_row.state.authority_contributors[0].association(), &association);
        let resumed = rows.iter().find(|row| row.state.input_id == input).unwrap();
        assert_eq!(&resumed.state.retained_resume.as_ref().unwrap().identity,
            &adapter.binding(&obligation).unwrap().retained_work);
        assert_eq!(fixture.cold_source_record().await, source_record);
        let state = fixture.continuation_native().agent_mob_mcp_state().unwrap();
        let service = state.continuation_service().unwrap();
        let owner = InstalledNativeContinuation::owner(adapter.binding(&obligation).unwrap());
        let key = adapter.delivery(&obligation).unwrap().key;
        assert!(matches!(service.continuation_status(&owner, &key).await.unwrap(),
            ContinuationStatus::Applied { receipt: observed, .. } if observed == receipt));
    }))).catch_unwind().await;
    if let Some(gate) = selected_gate.get() { gate.release.add_permits(1); }
    let mut cleanup = if let Some(fixture) = active.as_ref() {
        fixture.source.stop_custody_recovery();
        let (native, source) = tokio::join!(native_close.join(), fixture.source.join_custody_recovery());
        native.and(source.map_err(|error| error.to_string()))
    } else { Ok(()) };
    if let Some(gate) = selected_gate.get()
        && gate.cancelled.load(Ordering::SeqCst) != 0
    { cleanup = cleanup.and(Err("the entered census read was cancelled".into())); }
    drop(continuation);
    drop(active);
    drop(runtime_store);
    match checked {
        Err(payload) => {
            if let Err(error) = cleanup { eprintln!("census wake cleanup also failed: {error}"); }
            std::panic::resume_unwind(payload);
        },
        Ok(result) => {
            assert!(cleanup.is_ok(), "{cleanup:?}");
            assert!(result.is_ok(), "actual terminal census wake observation timed out");
        },
    }
}
