//! Actual helper payload retirement over the same installed/native pair.
//! Native admission is real; only physical model output and a selected failed
//! terminal source write are controlled. Restoration is a component callback,
//! not a second helper submission, continuation delivery or process-kill claim.
use super::*;
use toolkit_store::*;
use crate::original_ingress::custody_attempt::{CustodyAttempt, CustodyDecision};
use meerkat_runtime::{Input, input_state::{InputLifecycleState, verify_exact_replay}};
use std::sync::Mutex;

type ColdHelper = FirstFixture<meerkat::PersistentSessionService<FactoryAgentBuilder>>;

struct HelperStore {
    inner: Arc<dyn ToolkitStore>,
    fail_terminal: bool,
    terminal_calls: AtomicUsize,
}
#[async_trait::async_trait]
impl ToolkitStore for HelperStore {
    fn capabilities(&self) -> StoreCapabilities { self.inner.capabilities() }
    async fn now_ms(&self) -> StoreResult<i64> { self.inner.now_ms().await }
    async fn admit(&self, request: AdmissionRequest) -> StoreResult<AdmissionOutcome> { self.inner.admit(request).await }
    async fn get(&self, key: &StoreKey) -> StoreResult<Option<StoredRecord>> { self.inner.get(key).await }
    async fn list_records(&self, scope: &StoreScope, after: Option<&str>, limit: u32) -> StoreResult<Vec<StoredRecord>> {
        self.inner.list_records(scope, after, limit).await
    }
    async fn compare_and_swap(&self, transition: Transition) -> StoreResult<StoredRecord> {
        if self.fail_terminal && transition.data.schema == "meerkat_toolkit.helper_original"
            && transition.data.body["custody_attempt"]["decision"] == "promoted" {
            self.terminal_calls.fetch_add(1, Ordering::SeqCst);
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

#[derive(Clone, Copy)]
enum Case { Pending, Promoted, PendingTampered }

fn attempt(row: &StoredRecord) -> CustodyAttempt {
    serde_json::from_value(row.data.body["custody_attempt"].clone()).unwrap()
}
async fn change_body(fixture: &ColdHelper, before: &StoredRecord, data: RecordData) -> StoredRecord {
    fixture.store.compare_and_swap(Transition {
        key: before.key.clone(), expected_revision: before.revision,
        expected_digest: Some(before.content_digest.clone()), data,
        claim: None, events: vec![], effects: vec![],
    }).await.unwrap()
}
fn tampered_data(row: &StoredRecord) -> RecordData {
    let mut data = row.data.clone();
    let mut input: Input = serde_json::from_value(data.body["bound_input"].clone()).unwrap();
    let Input::Prompt(prompt) = &mut input else { panic!("actual helper prompt") };
    prompt.content = meerkat_core::ContentInput::Text("changed canonical helper payload".into());
    data.body["bound_input"] = serde_json::to_value(&input).unwrap();
    // Deliberately repair the source fingerprint as well as the real store
    // digest. Native's retained replay witness must still reject these bytes.
    data.body["custody_attempt"]["prepared_input_sha256"] = serde_json::json!(digest(
        &canonical_json(&("toolkit.prepared-original-input.v1", &input)).unwrap()));
    data
}

async fn exercise(case: Case) {
    let first_script = acquisition_denial::Script::paused_for_helper_entry();
    let fault = Arc::new(DeliveryFault::new(DeliveryMode::HoldHelperAcceptedReply));
    let selected = Arc::new(Mutex::new(None));
    let selected_in = selected.clone();
    let fail_terminal = !matches!(case, Case::Promoted);
    let (fixture, history, retained) = ColdHelper::for_durable_helper_recovery(
        first_script.clone(), Some(fault.clone()), None, Some(Box::new(move |inner| {
            let wrapper = Arc::new(HelperStore { inner, fail_terminal, terminal_calls: AtomicUsize::new(0) });
            *selected_in.lock().unwrap() = Some(wrapper.clone());
            wrapper
        })),
    ).await;
    let mut wrapper = selected.lock().unwrap().take();
    drop(selected);
    let mut active = Some(Arc::new(fixture));
    let mut history = Some(history);
    let mut carrier = Some(retained);
    let mut script = Some(first_script);
    let mut close = Some(active.as_ref().unwrap().retained_close_for_test());
    let mut release_round = Some(active.as_ref().unwrap().source.hold_first_custody_round_for_test());
    // Own the actual reopen future outside the body observer. Its caught
    // outcome is removed before use, so a completed panic is never repolled.
    let mut reopen = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 6, Box::pin(async {
        let (a_association, helper_association, helper_input, helper_row, native_before, a_before, grants, helper_session) = {
            let fixture = active.as_ref().unwrap();
            assert!(fixture.source.owner.has_installed_pair_recovery().unwrap());
            fixture.source.start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer()).unwrap();
            acquisition_approved::select_reviewed_setup(fixture).await;
            let first = fixture.submit().await;
            assert!(matches!(&first, Ok(PrivateOriginalSubmission::Admitted {
                deduplicated: false, source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
            })), "{}", submission_context(&first));
            script.as_ref().unwrap().first_response_waiting().await;
            script.as_ref().unwrap().release_first_response();
            fault.entered.notified().await;
            let held = fixture.source.helper_sources.lock().unwrap().first().cloned().unwrap();
            let pending = held.first_custody.lock().unwrap().clone().unwrap();
            assert!(pending.handoff.enrolled());
            let input = pending.input.clone();
            let association = held.original().association().clone();
            let helper_session = pending.session.clone();
            let helper_key = held.source_record().key.clone();
            let enrolled = fixture.store.get(&helper_key).await.unwrap().unwrap();
            assert_eq!(enrolled.data.version, 5);
            assert!(attempt(&enrolled).decision() == CustodyDecision::Pending);
            assert_eq!(enrolled.data.body["bound_input"], serde_json::to_value(&input).unwrap());
            assert!(attempt(&enrolled).matches_input(&input).unwrap());
            assert_eq!(wrapper.as_ref().unwrap().terminal_calls.load(Ordering::SeqCst), 0);
            fault.release.add_permits(1);
            fixture.finish(&first).await.unwrap();
            let consumed = acquisition_approved::consumed_native_input(history.as_ref().unwrap().as_ref(),
                &helper_session, &association, Some(&input.id().to_string()), tokio::time::Instant::now() + WAIT).await;
            fixture.machine.wait_run_settled(&helper_session, consumed.seed.last_run_id.as_ref().unwrap()).await;
            let receipt = pending.publication.lock().unwrap().native_admission.clone().unwrap();
            assert_eq!(receipt.session_id, helper_session);
            assert_eq!(&receipt.input_id, input.id());
            assert!(!receipt.deduplicated);
            assert!(fixture.source.owner.has_admitted_source(&association).unwrap());
            fixture.source.stop_custody_recovery();
            drop(release_round.take());
            let (native, source) = tokio::join!(close.as_mut().unwrap().join(), fixture.source.join_custody_recovery());
            assert!(native.is_ok(), "actual native/local close: {native:?}");
            if fail_terminal {
                assert!(matches!(source, Err(PrivateOriginalError::Unavailable)));
                assert!(wrapper.as_ref().unwrap().terminal_calls.load(Ordering::SeqCst) > 0);
            } else { assert!(source.is_ok(), "actual promoted close: {source:?}"); }
            let row = fixture.store.get(&helper_key).await.unwrap().unwrap();
            assert!(attempt(&row).decision() == if fail_terminal { CustodyDecision::Pending } else { CustodyDecision::Promoted });
            assert_eq!(row.data.body["bound_input"], enrolled.data.body["bound_input"]);
            let native = acquisition_approved::native_history(history.as_ref().unwrap().as_ref(), &helper_session).await;
            assert_eq!(native.len(), 1);
            assert!(native[0].seed.phase == InputLifecycleState::Consumed);
            assert!(native[0].state.persisted_input.is_none(), "actual Core retirement is required");
            verify_exact_replay(&native[0].state, &input).unwrap();
            let a_association = fixture.binding().await.unwrap().unwrap();
            let a_before = acquisition_approved::native_history(history.as_ref().unwrap().as_ref(), &fixture.session).await;
            assert_eq!(a_before.len(), 1);
            let grants = std::fs::read(fixture.source_path.parent().unwrap().join("grants.jsonl")).unwrap();
            // Carry only exact canonical evidence across owner death. The live
            // input above retains its process-only controller through ingress.
            let durable_input: Input = serde_json::from_value(row.data.body["bound_input"].clone()).unwrap();
            assert_eq!(serde_json::to_value(&durable_input).unwrap(), serde_json::to_value(&input).unwrap());
            assert_eq!(durable_input.id(), input.id());
            assert_eq!(durable_input.header().authority_association.as_ref(), Some(&association));
            assert!(attempt(&row).matches_input(&durable_input).unwrap());
            verify_exact_replay(&native[0].state, &durable_input).unwrap();
            assert!(durable_input.header().ingress_context.is_none());
            assert!(durable_input.header().retained_resume.is_none());
            (a_association, association, durable_input, row, native, a_before, grants, helper_session)
        };
        let old = active.as_ref().unwrap();
        let weak_source = Arc::downgrade(&old.source);
        let weak_machine = Arc::downgrade(&old.machine);
        let weak_service = Arc::downgrade(&old.service);
        let weak_lifetime = Arc::downgrade(old.lifetime());
        let weak_history = Arc::downgrade(history.as_ref().unwrap());
        // The fixture's scripted client otherwise retains its acquisition
        // owner back through the admitted controller. All real joins completed.
        script.as_ref().unwrap().release_owner_after_close();
        drop(close.take());
        drop(wrapper.take());
        drop(script.take());
        drop(active.take());
        drop(history.take());
        let first_owner_dead = (weak_source.upgrade().is_none(), weak_machine.upgrade().is_none(),
            weak_service.upgrade().is_none(), weak_lifetime.upgrade().is_none(), weak_history.upgrade().is_none());
        assert!(first_owner_dead.0 && first_owner_dead.1 && first_owner_dead.2
            && first_owner_dead.3 && first_owner_dead.4,
            "all first native/source owners must die: source_dead={} source_strong={} machine_dead={} machine_strong={} service_dead={} service_strong={} lifetime_dead={} lifetime_strong={} history_dead={} history_strong={}",
            first_owner_dead.0, weak_source.strong_count(), first_owner_dead.1, weak_machine.strong_count(),
            first_owner_dead.2, weak_service.strong_count(), first_owner_dead.3, weak_lifetime.strong_count(),
            first_owner_dead.4, weak_history.strong_count());

        let next_script = acquisition_denial::Script::paused_for_helper_entry();
        let retained = carrier.take();
        reopen = Some(Box::pin(std::panic::AssertUnwindSafe(async move {
            let fixture = ColdHelper::for_durable_helper_recovery(
                next_script.clone(), None, retained, None).await;
            (fixture, next_script)
        }).catch_unwind()));
        let opened = reopen.as_mut().unwrap().as_mut().await;
        drop(reopen.take());
        let ((fixture, next_history, retained), next_script) = match opened {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        active = Some(Arc::new(fixture));
        close = Some(active.as_ref().unwrap().retained_close_for_test());
        history = Some(next_history);
        carrier = Some(retained);
        script = Some(next_script);
        let fixture = active.as_ref().unwrap();
        assert_eq!(fixture.model_requests(), 0, "reopen must not submit or execute a model");
        assert_eq!(fixture.store.get(&helper_row.key).await.unwrap().as_ref(), Some(&helper_row));
        assert!(!fixture.source.owner.has_admitted_source(&helper_association).unwrap());

        if matches!(case, Case::PendingTampered) {
            let changed = change_body(fixture, &helper_row, tampered_data(&helper_row)).await;
            fixture.source.start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer()).unwrap();
            // Wait for the actual cold read and failed replay check to finish,
            // then close through the same retained task and cached error.
            loop {
                let row = fixture.store.get(&helper_row.key).await.unwrap().unwrap();
                if attempt(&row).observation_entered() { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            fixture.source.stop_custody_recovery();
            assert!(matches!(fixture.source.join_custody_recovery().await, Err(PrivateOriginalError::Unavailable)));
            let row = fixture.store.get(&helper_row.key).await.unwrap().unwrap();
            assert!(attempt(&row).decision() == CustodyDecision::Pending);
            assert_eq!(row.data.body["bound_input"], changed.data.body["bound_input"]);
            let failures = fixture.source.retained_cold_failures_for_test().await;
            assert!(failures.iter().any(|(key, observed)| key == &helper_row.key
                && observed.as_ref().is_some_and(|state| state.state.input_id == *helper_input.id())));
            assert!(!fixture.source.owner.has_admitted_source(&helper_association).unwrap());
            assert!(matches!(fixture.source.join_custody_recovery().await, Err(PrivateOriginalError::Unavailable)));
            assert_eq!(fixture.store.get(&helper_row.key).await.unwrap().as_ref(), Some(&row));
        } else {
            fixture.source.start_custody_recovery(fixture.native.mob_runtime().continuation_source_announcer()).unwrap();
            loop {
                let row = fixture.store.get(&helper_row.key).await.unwrap().unwrap();
                if attempt(&row).decision() == CustodyDecision::Promoted { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let promoted = fixture.store.get(&helper_row.key).await.unwrap().unwrap();
            assert_eq!(promoted.data.body["bound_input"], helper_row.data.body["bound_input"]);
            if matches!(case, Case::Promoted) { assert_eq!(promoted, helper_row); }
            let a = fixture.bind_existing_source_for_recovery_test(&a_association).await;
            fixture.source.restore_promoted_custody(&fixture.cold_source_record().await.key, &a).await.unwrap();
            assert!(fixture.source.owner.has_admitted_source(&a_association).unwrap());
            let bound = fixture.source.owner.bind_cold_helper_for_recovery_test(
                fixture.source.store.as_ref(), &fixture.scope, &promoted).unwrap();
            assert_eq!(bound.original.association(), &helper_association);
            assert!(!bound.original.first_binding_for(&promoted.key));
            assert!(!fixture.source.owner.has_admitted_source(&helper_association).unwrap());
            if matches!(case, Case::Promoted) {
                let changed = change_body(fixture, &promoted, tampered_data(&promoted)).await;
                assert!(matches!(fixture.source.restore_promoted_custody(&changed.key, &bound.original).await,
                    Err(PrivateOriginalError::Conflict)));
                assert!(!fixture.source.owner.has_admitted_source(&helper_association).unwrap());
                let mut legacy = promoted.data.clone();
                legacy.version = 4;
                legacy.body.as_object_mut().unwrap().remove("bound_input");
                let legacy = change_body(fixture, &changed, legacy).await;
                assert!(matches!(fixture.source.restore_promoted_custody(&legacy.key, &bound.original).await,
                    Err(PrivateOriginalError::Unavailable)));
                assert!(!fixture.source.owner.has_admitted_source(&helper_association).unwrap());
                change_body(fixture, &legacy, promoted.data.clone()).await;
            }
            let before_restore = fixture.store.get(&promoted.key).await.unwrap().unwrap();
            fixture.source.restore_promoted_custody(&promoted.key, &bound.original).await.unwrap();
            assert!(fixture.source.owner.has_admitted_source(&helper_association).unwrap());
            assert_eq!(fixture.store.get(&promoted.key).await.unwrap().as_ref(), Some(&before_restore));
            fixture.source.stop_custody_recovery();
            assert!(fixture.source.join_custody_recovery().await.is_ok());
        }
        assert_eq!(serde_json::to_value(acquisition_approved::native_history(history.as_ref().unwrap().as_ref(), &helper_session).await).unwrap(),
            serde_json::to_value(&native_before).unwrap());
        assert_eq!(serde_json::to_value(acquisition_approved::native_history(history.as_ref().unwrap().as_ref(), &fixture.session).await).unwrap(),
            serde_json::to_value(&a_before).unwrap());
        assert_eq!(std::fs::read(fixture.source_path.parent().unwrap().join("grants.jsonl")).unwrap(), grants);
        assert_eq!(fixture.model_requests(), 0);
        assert!(close.as_mut().unwrap().join().await.is_ok());
    }))).catch_unwind().await;

    drop(release_round.take());
    fault.release.add_permits(1);
    if let Some(script) = &script { script.release_first_response(); }
    let mut reopen_panicked = false;
    if let Some(mut opening) = reopen.take() {
        // The outer observer may have expired in startup or its own failure
        // cleanup. Join that same future before closing any returned owner.
        match opening.as_mut().await {
            Ok(((fixture, next_history, retained), next_script)) => {
                active = Some(Arc::new(fixture));
                close = Some(active.as_ref().unwrap().retained_close_for_test());
                history = Some(next_history);
                carrier = Some(retained);
                next_script.release_first_response();
                script = Some(next_script);
            }
            Err(_) => reopen_panicked = true,
        }
    }
    let cleanup = if let Some(fixture) = &active {
        fixture.source.stop_custody_recovery();
        let (native, source) = tokio::join!(close.as_mut().unwrap().join(), fixture.source.join_custody_recovery());
        Some((native, source))
    } else { None };
    // Also release the fixture backedge after failed bodies and returned reopen
    // owners. Preserve every actual close result above for the assertions below.
    if let Some(script) = &script { script.release_owner_after_close(); }
    match checked {
        Ok(Ok(())) => {
            assert!(!reopen_panicked, "retained reopen panicked after the body observer");
            let (native, source) = cleanup.unwrap();
            assert!(native.is_ok(), "actual native/local cleanup: {native:?}");
            assert!(if matches!(case, Case::PendingTampered) {
                matches!(source, Err(PrivateOriginalError::Unavailable))
            } else { source.is_ok() }, "actual source cleanup: {source:?}");
        }
        Ok(Err(_)) => panic!("cold helper control timed out; reopen_panicked={reopen_panicked}; cleanup={cleanup:?}"),
        Err(payload) => { eprintln!("cold helper cleanup after primary failure: reopen_panicked={reopen_panicked}; cleanup={cleanup:?}"); std::panic::resume_unwind(payload); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_pending_helper_settles_retired_input_without_resubmission() { Box::pin(exercise(Case::Pending)).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_promoted_helper_restores_exact_input_and_refuses_tampered_or_legacy_bytes() { Box::pin(exercise(Case::Promoted)).await; }
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_pending_helper_retains_native_evidence_when_source_payload_is_tampered() { Box::pin(exercise(Case::PendingTampered)).await; }
