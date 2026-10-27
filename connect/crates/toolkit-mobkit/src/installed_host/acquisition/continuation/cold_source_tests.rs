//! Actual source admission, canonical acquisition and native continuation owner.
//! The original controls simulate process-custody loss. The final component
//! control closes and reopens durable owners in one process. Neither exercises
//! a protected Console login; both require the selected #35/Meta forwarding.
use super::*;
use meerkat_core::retained_work::RetainedWorkIdentity;
use toolkit_store::{ToolkitStore, Transition};

#[derive(Clone, Copy, Debug)]
enum Case { First, Pending, TargetedFirst, TargetedPending, MissingInput, ChangedInput, Cancelled, ExtraContributor,
    ExtraSelected, BadRun, BadSubmission, WarmLegacy, CancelWhileObserved }

fn changed_work(work: &RetainedWorkIdentity, case: Case) -> RetainedWorkIdentity {
    let mut contributors = work.contributors().to_vec();
    let mut selected = work.selected_input_bindings().clone();
    let mut run = work.run_id().clone();
    match case {
        Case::ExtraContributor => contributors.push(contributors[0].clone()),
        Case::ExtraSelected => { selected.insert("another-selected-input".into(), selected.values().next().unwrap().clone()); },
        Case::BadRun => run = meerkat_core::lifecycle::RunId::new(),
        Case::BadSubmission => contributors[0].submission_digest = "0".repeat(64),
        _ => (),
    }
    RetainedWorkIdentity::new(work.runtime_id(), run, contributors, selected, work.controller().cloned())
}

async fn case(explicit: bool, case: Case) {
    let fixture = if matches!(case, Case::WarmLegacy) { ContinuationFixture::for_continuation().await }
        else { ContinuationFixture::for_cold_continuation(explicit, matches!(case, Case::TargetedFirst | Case::TargetedPending)).await };
    let pins = Arc::new(PausedPins { inner: fixture.service.clone(),
        pause: matches!(case, Case::CancelWhileObserved), entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new() });
    if !matches!(case, Case::WarmLegacy) { fixture.source.bind_controller_pin_source(pins.clone()).unwrap(); }
    let mut owner_handle = None;
    let mut owner_passes = None;
    let result = fixture.source.with_synthetic_admission_diagnostics(AssertUnwindSafe(Box::pin(async {
        let admitted = fixture.submit().await;
        fixture.finish(&admitted).await.unwrap();
        let context = fixture.probe.as_ref().unwrap().captured();
        tokio::time::timeout(EVENT_GUARD, fixture.machine.wait_run_settled(
            &fixture.session, context.retained_work().unwrap().run_id())).await.unwrap();
        assert_eq!(fixture.inputs().await, Ok(1));
        let association = fixture.binding().await.unwrap().unwrap();
        if matches!(case, Case::TargetedFirst | Case::TargetedPending) {
            assert!(matches!(&association.candidate().authority_basis,
                meerkat_authorization_contracts::work_association::WorkAuthorityBasis::TargetedGrantLineages { targets, .. }
                    if targets.len() == 2));
        }
        let source_before = fixture.cold_source_record().await;
        if matches!(case, Case::WarmLegacy) {
            assert!(source_before.data.body.get("bound_service_input").is_none(),
                "unsupported warm source does not enter the extra evidence write");
        } else {
            assert!(!source_before.data.body["bound_service_input"].is_null(),
                "actual binder retained the input before native admission");
            assert!(source_before.data.body[if explicit { "setup_request" } else { "configured_setup" }].is_object());
        }
        let adapter = InstalledNativeContinuation::new(fixture.store.clone(), fixture.scope.clone(),
            fixture.source.clone(), id("installed-regression"), [id("person")].into()).unwrap();
        let state = MobMcpState::new_with_runtime_adapter(fixture.service.clone(), Some(fixture.machine.clone()),
            meerkat_mob::MobControlPrincipal::Owner).unwrap()
            .with_host_job_source(HostJobSource::new([adapter.namespace()], adapter.clone()).unwrap()).into_shared();
        let inbox = RuntimeDeliveryInbox::new(Arc::new(InMemoryRuntimeStore::new()));
        let bindings = Arc::new(ContinuationHostBindings::default());
        state.bind_continuations(inbox.clone(), &bindings).unwrap();
        let service = state.continuation_service().unwrap();
        adapter.bind_fixture_native(service.clone(), state.clone()).unwrap();
        let selected_work = changed_work(context.retained_work().unwrap(), case);
        let obligation = terminal_result_with_work(&adapter, &fixture, &context, Some(selected_work)).await;
        let binding = adapter.binding(&obligation).unwrap();
        let owner = InstalledNativeContinuation::owner(binding);
        let key = adapter.delivery(&obligation).unwrap().key;
        if matches!(case, Case::Pending | Case::TargetedPending) {
            let job = RetainedJobRecord::from_owner(adapter.as_ref(), adapter.producer(), obligation.operation().as_str())
                .await.unwrap().unwrap();
            service.submit_retained_completion(&owner, adapter.delivery(&obligation).unwrap(), &job,
                u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap()).await.unwrap();
        }
        if !matches!(case, Case::WarmLegacy) { fixture.forget_source_custody(&association); }
        if matches!(case, Case::MissingInput | Case::ChangedInput) {
            let mut data = source_before.data.clone();
            if matches!(case, Case::MissingInput) {
                data.body.as_object_mut().unwrap().remove("bound_service_input");
            } else {
                // Corrupt only the actual bound input evidence. Never replace
                // the native row or manufacture a matching replay witness.
                data.body["bound_service_input"]["content"] = serde_json::json!("changed input");
            }
            fixture.store.compare_and_swap(Transition { key: source_before.key.clone(), expected_revision: source_before.revision,
                expected_digest: Some(source_before.content_digest.clone()), claim: None, data, events: vec![], effects: vec![],
            }).await.unwrap();
        }
        if matches!(case, Case::Cancelled) {
            fixture.source.record_operation_cancelled(fixture.original_reference()).await.unwrap();
        }
        let calls = fixture.model_requests();
        if matches!(case, Case::CancelWhileObserved) {
            let observation = fixture.source.restore_setup_continuation_source(&obligation);
            tokio::pin!(observation);
            tokio::select! {
                biased;
                _ = pins.entered.notified() => (),
                result = &mut observation => panic!("observation bypassed the real actor boundary: {result:?}"),
                _ = tokio::time::sleep(EVENT_GUARD) => panic!("actor observation did not arrive"),
            }
            // The read phase released the source writer before this native
            // observation wait. Cancellation must enter and prevent promotion.
            fixture.source.record_operation_cancelled(fixture.original_reference()).await.unwrap();
            pins.release.notify_one();
            assert!(matches!(tokio::time::timeout(EVENT_GUARD, &mut observation).await.unwrap(),
                Err(ContinuationError::OwnerUnavailable | ContinuationError::Denied)));
            assert!(fixture.cold_source_custody(&association).is_none());
            assert_eq!(fixture.inputs().await, Ok(1));
            assert_eq!(fixture.model_requests(), calls);
            return;
        }
        let attempt = if matches!(case, Case::Pending | Case::TargetedPending) {
            assert!(matches!(adapter.reconcile(&obligation).await, Ok(ContinuationReconciliation::Pending { .. })));
            Ok(())
        } else { adapter.admit(&obligation).await.map(|_| ()) };
        if matches!(case, Case::MissingInput | Case::ChangedInput | Case::Cancelled | Case::ExtraContributor | Case::ExtraSelected) {
            assert!(attempt.is_err());
            assert!(fixture.cold_source_custody(&association).is_none(), "refusal must not promote the anchor alone");
            assert!(matches!(service.continuation_status(&owner, &key).await.unwrap(), ContinuationStatus::NotCommitted));
            assert_eq!(fixture.inputs().await, Ok(1));
            assert_eq!(fixture.model_requests(), calls);
            return;
        }
        if !matches!(case, Case::Pending | Case::TargetedPending) {
            assert!(matches!(&attempt, Err(ContinuationError::Timeout)),
                "the existing native inbox commit must precede its not-yet-armed drain: {attempt:?}");
        }
        assert!(matches!(service.continuation_status(&owner, &key).await.unwrap(),
            ContinuationStatus::Pending { admitted: None, .. }),
            "the exact native row is Pending before the actual delivery owner is armed");
        if !matches!(case, Case::WarmLegacy) {
            assert_eq!(fixture.cold_source_custody(&association), Some((true, true)),
                "native historical observation restores Task without a revoke issuer");
        }
        assert_eq!(fixture.inputs().await, Ok(1), "custody recovery never resubmits A");
        assert_eq!(fixture.model_requests(), calls);
        assert_eq!(fixture.cold_source_record().await.content_digest, source_before.content_digest,
            "native observation does not rewrite saved ID/time, setup choice or source facts");
        let jobs = Arc::new(MemoryDetachedJobStore::new());
        let host = SessionServiceDeliveryHost::new(&fixture.service, &fixture.machine,
            DetachedJobService::new(jobs.clone()), None).with_continuation_bindings(bindings.clone());
        let handle = RuntimeDeliveryOwner::new(jobs, inbox)
            .with_source_changes(bindings.subscribe_source_changes())
            .arm(Arc::new(host)).unwrap();
        owner_passes = Some(handle.subscribe_passes());
        owner_handle = Some(handle);
        if matches!(case, Case::BadRun | Case::BadSubmission) {
            // Historical source custody is insufficient for altered native run
            // or replay evidence. Exercise the actual retained-completion drain.
            let passes = owner_passes.as_mut().unwrap();
            let pass = tokio::time::timeout(EVENT_GUARD, async {
                loop {
                    let observed = passes.borrow_and_update().clone();
                    if observed.generation > 0 { break observed; }
                    passes.changed().await.unwrap();
                }
            }).await.unwrap();
            assert!(!pass.failures.is_empty(), "the actual drain must report its rejected retained evidence: {pass:?}");
            assert!(!matches!(service.continuation_status(&owner, &key).await.unwrap(), ContinuationStatus::Applied { .. }));
            assert_eq!(fixture.inputs().await, Ok(1));
            assert_eq!(fixture.model_requests(), calls);
            return;
        }
        let status = wait_applied(&service, &owner, &key, owner_passes.as_mut().unwrap()).await;
        let ContinuationStatus::Applied { session, input, .. } = status else { panic!("native applied receipt") };
        let row = fixture.machine.input_state(&session, &input).await.unwrap().unwrap();
        assert_eq!(&row.state.retained_resume.as_ref().unwrap().identity, &binding.retained_work);
        assert_eq!(fixture.inputs().await, Ok(2), "only the exact retained completion is newly admitted");
        assert!(matches!(adapter.reconcile(&obligation).await, Ok(ContinuationReconciliation::Applied(_))));
        assert_eq!(fixture.inputs().await, Ok(2));
    })).catch_unwind()).await;
    drop(owner_handle);
    if let Some(mut passes) = owner_passes {
        tokio::time::timeout(EVENT_GUARD, async { while passes.changed().await.is_ok() {} }).await.unwrap();
    }
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_setup_a_first_and_pending_continuation_recover_without_resubmitting_a() {
    for explicit in [false, true] { for mode in [Case::First, Case::Pending] { Box::pin(case(explicit, mode)).await; } }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_setup_a_missing_changed_cancelled_or_incomplete_source_refuses_before_commit() {
    for mode in [Case::MissingInput, Case::ChangedInput, Case::Cancelled, Case::ExtraContributor, Case::ExtraSelected] {
        Box::pin(case(false, mode)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_setup_a_historical_custody_cannot_validate_another_run_or_submission() {
    for mode in [Case::BadRun, Case::BadSubmission] { Box::pin(case(false, mode)).await; }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warm_legacy_continuation_needs_no_new_cold_evidence() {
    Box::pin(case(false, Case::WarmLegacy)).await;
}

/// Hold only delivery of the actual actor observation, without acquiring B or
/// inventing its identity. Native's own controls exercise B-lock contention.
struct PausedPins {
    inner: Arc<dyn meerkat_mobkit::identity_first::ControllerPinSource>,
    pause: bool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait]
impl meerkat_mobkit::identity_first::ControllerPinSource for PausedPins {
    async fn live_actor(&self, session: &SessionId) -> Option<meerkat::LiveSessionActorWitness> {
        let actor = self.inner.live_actor(session).await;
        if self.pause { self.entered.notify_one(); self.release.notified().await; }
        actor
    }
    async fn pin_actor(&self, _: &meerkat::LiveSessionActorWitness)
        -> Result<meerkat_core::ControllerModelClient, String>
    { panic!("the cold caller must not acquire B through pin_actor"); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_setup_a_cancellation_can_enter_during_actor_wait_and_refuses_promotion() {
    Box::pin(case(false, Case::CancelWhileObserved)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_setup_a_targeted_selection_restores_its_exact_operation_projection() {
    for mode in [Case::TargetedFirst, Case::TargetedPending] { Box::pin(case(false, mode)).await; }
}

/// Component acceptance for durable-owner reopen through actual Meta delivery.
/// The selected owner must first report the cold source refusal, then drain its
/// existing Pending row after Toolkit restores custody and publishes its hint.
/// No new original, completion, attachment, owner or manual wake is supplied.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_owner_reopen_drains_same_pending_completion_after_source_recovery() {
    use meerkat_runtime::{LogicalRuntimeId, input_state::InputLifecycleState};
    let (fixture, store, retained, adapter) = ContinuationFixture::for_durable_continuation().await;
    let mut active = Some(fixture);
    let mut runtime_store = Some(store);
    let mut retained = Some(retained);
    let mut continuation = Some(adapter);
    let result = AssertUnwindSafe(Box::pin(async {
        // Keep the whole first composition inside this scope. Only canonical
        // values and filesystem selections cross the joined close/drop below.
        let (obligation, receipt, association, source_digest, original_input, session) = {
            let fixture = active.as_ref().unwrap();
            let adapter = continuation.as_ref().unwrap();
            let admitted = fixture.submit().await;
            fixture.finish(&admitted).await.unwrap();
            let context = fixture.probe.as_ref().unwrap().captured();
            tokio::time::timeout(EVENT_GUARD, fixture.machine.wait_run_settled(
                &fixture.session, context.retained_work().unwrap().run_id())).await.unwrap();
            let association = fixture.binding().await.unwrap().unwrap();
            let rows = runtime_store.as_ref().unwrap().load_input_states_strict(
                &LogicalRuntimeId::for_session(&fixture.session)).await.unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].seed.phase, InputLifecycleState::Consumed);
            let original_input = rows[0].state.input_id.clone();
            let source = fixture.cold_source_record().await;
            assert!(source.data.body["configured_setup"].is_object());
            assert!(!source.data.body["bound_service_input"].is_null());
            let state = fixture.continuation_native().agent_mob_mcp_state().unwrap();
            let service = state.continuation_service().unwrap();
            let obligation = terminal_result(adapter, fixture, &context).await;
            let owner = InstalledNativeContinuation::owner(adapter.binding(&obligation).unwrap());
            let delivery = adapter.delivery(&obligation).unwrap();
            let job = RetainedJobRecord::from_owner(adapter.as_ref(), adapter.producer(), obligation.operation().as_str())
                .await.unwrap().unwrap();
            // The already-armed native owner must not admit before the first
            // close. Lose only process custody before committing this result.
            fixture.forget_source_custody(&association);
            let receipt = service.submit_retained_completion(&owner, delivery.clone(), &job,
                u64::try_from(fixture.store.now_ms().await.unwrap()).unwrap()).await.unwrap();
            assert!(matches!(service.continuation_status(&owner, &delivery.key).await.unwrap(),
                ContinuationStatus::Pending { receipt: ref observed, admitted: None, .. } if observed == &receipt));
            (obligation, receipt, association, source.content_digest, original_input, fixture.session.clone())
        };
        let weak_source = Arc::downgrade(&active.as_ref().unwrap().source);
        let weak_machine = Arc::downgrade(&active.as_ref().unwrap().machine);
        let weak_service = Arc::downgrade(&active.as_ref().unwrap().service);
        let weak_lifetime = Arc::downgrade(active.as_ref().unwrap().lifetime());
        let weak_store = Arc::downgrade(runtime_store.as_ref().unwrap());
        let weak_adapter = Arc::downgrade(continuation.as_ref().unwrap());
        active.as_ref().unwrap().close_durable_owners().await.unwrap();
        drop(continuation.take());
        drop(active.take());
        drop(runtime_store.take());
        assert!(weak_adapter.upgrade().is_none(), "first continuation source still retained");
        assert!(weak_source.upgrade().is_none(), "first source owner still retained");
        assert!(weak_machine.upgrade().is_none(), "first machine still retained");
        assert!(weak_service.upgrade().is_none(), "first service still retained");
        assert!(weak_lifetime.upgrade().is_none(), "first installed owner still retained");
        assert!(weak_store.upgrade().is_none(), "first runtime store still retained");

        let (fixture, store, adapter) = ContinuationFixture::reopen_durable_continuation(retained.take().unwrap()).await;
        active = Some(fixture);
        runtime_store = Some(store);
        continuation = Some(adapter);
        let fixture = active.as_ref().unwrap();
        let adapter = continuation.as_ref().unwrap();
        assert_eq!(fixture.session, session, "native restore keeps the same original session");
        assert_eq!(fixture.model_requests(), 0, "reopening did not replay A");
        assert!(fixture.cold_source_custody(&association).is_none(), "no process custody crosses reopen");
        assert_eq!(fixture.cold_source_record().await.content_digest, source_digest);
        let address = LogicalRuntimeId::for_session(&session);
        let rows = runtime_store.as_ref().unwrap().load_input_states_strict(&address).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state.input_id, original_input);
        assert_eq!(rows[0].seed.phase, InputLifecycleState::Consumed);
        let state = fixture.continuation_native().agent_mob_mcp_state().unwrap();
        let service = state.continuation_service().unwrap();
        let owner = InstalledNativeContinuation::owner(adapter.binding(&obligation).unwrap());
        let key = adapter.delivery(&obligation).unwrap().key;

        // The runtime already selected and armed this owner. Wait only on its
        // read-only pass projection; a status resolver must not revive an actor
        // and accidentally provide the signal this test is meant to require.
        let blocked = tokio::time::timeout(EVENT_GUARD, async {
            loop {
                if let Some(pass) = fixture.continuation_native().runtime_delivery_pass()
                    && pass.generation > 0 && pass.blocked_sessions.contains(&session)
                { break pass; }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert_eq!(blocked.applied, 0);
        assert_eq!(blocked.refused, 0);
        assert!(!blocked.failures.is_empty(), "the real owner must have attempted the retained completion");
        assert!(fixture.cold_source_custody(&association).is_none());
        // Native reserves an input ID before trying admission. A retained
        // reservation is not an admitted input and must survive this retry.
        let reserved_input = match runtime_store.as_ref().unwrap()
            .load_continuation_admission(&address, &receipt.delivery_id).await.unwrap() {
            Some(meerkat_runtime::ContinuationAdmission::Reserved { session_id, input_id }) => {
                assert_eq!(session_id, session);
                assert_ne!(input_id, original_input);
                input_id
            },
            other => panic!("blocked continuation must retain its reservation: {other:?}"),
        };
        let rows = runtime_store.as_ref().unwrap().load_input_states_strict(&address).await.unwrap();
        assert_eq!(rows.len(), 1, "blocked delivery has not admitted a continuation input");
        assert_eq!(rows[0].state.input_id, original_input);
        assert_eq!(rows[0].seed.phase, InputLifecycleState::Consumed);
        assert_eq!(fixture.model_requests(), 0);

        match fixture.source.with_synthetic_admission_diagnostics(adapter.reconcile(&obligation)).await {
            Ok(ContinuationReconciliation::Pending { .. }) => {},
            Ok(ContinuationReconciliation::Applied(_)) => panic!("expected Pending reconciliation, observed Applied"),
            Ok(ContinuationReconciliation::SafeToRetry { .. }) => panic!("expected Pending reconciliation, observed SafeToRetry"),
            Ok(ContinuationReconciliation::DefinitivelyRejected { .. }) => panic!("expected Pending reconciliation, observed DefinitivelyRejected"),
            Err(error) => panic!("expected Pending reconciliation, observed Err({error:?})"),
        }
        assert_eq!(fixture.cold_source_custody(&association), Some((true, true)));
        assert_eq!(fixture.cold_source_record().await.content_digest, source_digest);
        assert!(!fixture.source.restore_setup_continuation_source(&obligation).await.unwrap(),
            "warm custody is not another source-change hint");
        // Reconciliation performs its normal status resolution before restoring custody.
        // After reconciliation, the test waits using only durable admission reads.
        let (applied_session, input) = tokio::time::timeout(EVENT_GUARD, async {
            loop {
                if let Some(meerkat_runtime::ContinuationAdmission::Applied { session_id, input_id }) =
                    runtime_store.as_ref().unwrap().load_continuation_admission(&address, &receipt.delivery_id).await.unwrap()
                { break (session_id, input_id); }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        let applied = service.continuation_status(&owner, &key).await.unwrap();
        assert!(matches!(applied, ContinuationStatus::Applied { receipt: observed, .. } if observed == receipt));
        assert_eq!(applied_session, session);
        assert_eq!(input, reserved_input, "source recovery uses the same native reservation");
        assert_ne!(input, original_input);
        let rows = runtime_store.as_ref().unwrap().load_input_states_strict(&address).await.unwrap();
        assert_eq!(rows.len(), 2, "only the committed completion may add an input");
        let original = rows.iter().find(|row| row.state.input_id == original_input).unwrap();
        assert_eq!(original.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(original.state.authority_contributors[0].association(), &association);
        let resumed = rows.iter().find(|row| row.state.input_id == input).unwrap();
        assert_eq!(&resumed.state.retained_resume.as_ref().unwrap().identity,
            &adapter.binding(&obligation).unwrap().retained_work);
        assert!(matches!(adapter.reconcile(&obligation).await, Ok(ContinuationReconciliation::Applied(_))));
    })).catch_unwind().await;
    let cleanup = if let Some(fixture) = active.as_ref() { fixture.close_durable_owners().await } else { Ok(()) };
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[path = "custody_recovery_wake_tests.rs"]
mod custody_recovery_wake_tests;
