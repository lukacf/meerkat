//! Source-custody loss on the existing real native A/B fixture. This is not a
//! process restart or live MCP qualification. The selected native #35 source
//! and actual Meta under-B forwarding are required to execute these controls.
use super::*;
use crate::private_originals::governed_submit::tests::DeliveryMode;
use meerkat_mobkit::identity_first::ControllerPinSource;
use meerkat_runtime::Input;

async fn approved<S>(fixture: &ContinuationFixture<S>) -> PrivateOriginalRef {
    let a = fixture.submit().await;
    fixture.finish(&a).await.unwrap();
    let dispatch = fixture.probe.as_ref().unwrap().captured();
    let run = dispatch.retained_work().unwrap().run_id();
    tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(&fixture.session, run)).await.unwrap();
    approve_b_operations(fixture, acquisition(fixture, &dispatch).await, [id("read")].into()).await
}

async fn row<S>(fixture: &ContinuationFixture<S>, b: &PrivateOriginalRef) -> (StoredRecord, SourceRecord) {
    let stored = fixture.store.get(&fixture.source.key(b).unwrap()).await.unwrap().unwrap();
    let record = fixture.source.decode_record(&stored, b).unwrap();
    (stored, record)
}

async fn replace_row<S>(fixture: &ContinuationFixture<S>, stored: StoredRecord, record: SourceRecord) {
    fixture.store.compare_and_swap(Transition {
        key: stored.key, expected_revision: stored.revision,
        expected_digest: Some(stored.content_digest), claim: None, data: encode(record).unwrap(),
        events: vec![], effects: vec![],
    }).await.unwrap();
}

async fn admitted_without_ack<S>(fixture: &ContinuationFixture<S>, b: &PrivateOriginalRef) -> (SourceRecord, InputId) {
    let (stored, _) = row(fixture, b).await;
    let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
    sql.execute_batch(&format!(
        "CREATE TRIGGER fail_cold_ack BEFORE UPDATE OF data ON toolkit_records \
         WHEN NEW.id = '{}' AND json_extract(NEW.data, '$.body.service_admission') IS NOT NULL \
         BEGIN SELECT RAISE(ABORT, 'lost cold source acknowledgement'); END;",
        stored.key.id.replace('\'', "''"),
    )).unwrap();
    assert!(matches!(fixture.source.step_service_input(&stored).await, Err(PrivateOriginalError::Unavailable)));
    sql.execute_batch("DROP TRIGGER fail_cold_ack;").unwrap();
    let (_, record) = row(fixture, b).await;
    assert!(record.service_admission.is_none());
    let input = record.bound_service_input.as_ref().unwrap().decode().unwrap();
    let input_id = input.id().clone();
    let mut settlements = fixture.machine.subscribe_run_settlements();
    tokio::time::timeout(WAIT, async {
        loop {
            let actual = fixture.machine.input_state(&fixture.session, &input_id).await.unwrap().unwrap();
            if let Some(run) = &actual.seed.last_run_id {
                if (actual.seed.phase == meerkat_runtime::input_state::InputLifecycleState::Consumed
                    || fixture.machine.run_turn_terminal(&fixture.session, run).await == Some(TurnTerminalOutcome::Completed))
                    && fixture.machine.current_run(&fixture.session).await.as_ref() != Some(run) { break; }
            }
            settlements.changed().await.unwrap();
        }
    }).await.expect("actual B settles before the source-loss simulation");
    assert!(!fixture.source.service_task_submission_current(&record).await.unwrap(),
        "historical recovery must not reuse A's now overwritten fresh-entry witness");
    (record, input_id)
}

fn forget_b<S>(fixture: &ContinuationFixture<S>, b: &PrivateOriginalRef, record: &SourceRecord) {
    // Drop actual already-settled pending guards before removing only this B's
    // test-owned process row. Durable input/grants/source/parent remain intact.
    fixture.source.retained.lock().unwrap().retain(|held| held.reference != *b);
    fixture.source.owner.forget_task_custody_for_test(record.final_association.as_ref().unwrap());
    assert!(fixture.source.owner.historical_task_custody_for_test(record.final_association.as_ref().unwrap()).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_b_restores_actual_admission_and_phase_without_replay_or_new_issuer() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        fixture.source.bind_controller_pin_source(fixture.service.clone()).unwrap();
        let b = approved(&fixture).await;
        let (record, input_id) = admitted_without_ack(&fixture, &b).await;
        let association = record.final_association.as_ref().unwrap();
        let saved_input = serde_json::to_value(record.bound_service_input.as_ref().unwrap()).unwrap();
        let actual = fixture.machine.input_state(&fixture.session, &input_id).await.unwrap().unwrap();
        let requests = fixture.model_requests();
        let inputs = fixture.inputs().await.unwrap();
        // Semantic idempotence permits the next actual binder's transport ID,
        // but never overwrites the first retained ID/time or accepts new content.
        let held = fixture.source.retained.lock().unwrap().iter().find(|held| held.reference == b).unwrap().clone();
        let mut retry = record.bound_service_input.as_ref().unwrap().decode().unwrap();
        let Input::Prompt(prompt) = &mut retry else { panic!("bound prompt"); };
        prompt.header.id = InputId::new();
        fixture.source.retain_bound_service_input(&b, &held.original, &retry).await.unwrap();
        let Input::Prompt(prompt) = &mut retry else { unreachable!() };
        prompt.content = meerkat_core::ContentInput::Text("different instruction".into());
        assert!(matches!(fixture.source.retain_bound_service_input(&b, &held.original, &retry).await,
            Err(PrivateOriginalError::Conflict)));
        drop(held);
        forget_b(&fixture, &b, &record);
        let restored = fixture.source.restore_service_admission(&b).await.unwrap().unwrap();
        assert_eq!(restored.admission.input_id, input_id.to_string());
        assert_eq!(restored.admission.session_id, fixture.session.to_string());
        assert_eq!(restored.phase, actual.seed.phase, "historical phase is returned unchanged");
        assert_eq!(fixture.source.owner.historical_task_custody_for_test(association), Some((true, true)));
        let again = fixture.source.restore_service_admission(&b).await.unwrap().unwrap();
        assert_eq!(again.admission.input_id, restored.admission.input_id);
        let (stored, _) = row(&fixture, &b).await;
        fixture.source.step_service_input(&stored).await.unwrap();
        let (_, saved) = row(&fixture, &b).await;
        assert!(saved.service_admission == Some(restored.admission));
        assert_eq!(serde_json::to_value(saved.bound_service_input.as_ref().unwrap()).unwrap(), saved_input);
        assert_eq!(fixture.source.service_task_snapshot(&saved).await.unwrap().state().lifecycle_phase,
            toolkit_catalog::AcquisitionPhase::Ready);
        assert_eq!(fixture.inputs().await.unwrap(), inputs);
        assert_eq!(fixture.model_requests(), requests);
        assert_eq!(fixture.source.owner.historical_task_custody_for_test(association), Some((true, true)));
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_b_requires_complete_reserved_selection_and_retained_input() {
    for corruption in ["task-link", "legacy-no-input", "input-key", "input-content", "input-context"] {
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            fixture.source.bind_controller_pin_source(fixture.service.clone()).unwrap();
            let b = approved(&fixture).await;
            let (record, _) = admitted_without_ack(&fixture, &b).await;
            forget_b(&fixture, &b, &record);
            let inputs = fixture.inputs().await.unwrap();
            let requests = fixture.model_requests();
            let (stored, mut changed) = row(&fixture, &b).await;
            match corruption {
                "task-link" => changed.approved_task.as_mut().unwrap().header_mut().before_ms -= 1,
                "legacy-no-input" => changed.bound_service_input = None,
                _ => {
                    let mut input = changed.bound_service_input.as_ref().unwrap().decode().unwrap();
                    let Input::Prompt(prompt) = &mut input else { panic!("prompt"); };
                    if corruption == "input-key" {
                        prompt.header.idempotency_key = Some(meerkat_runtime::identifiers::IdempotencyKey::new("other"));
                    } else if corruption == "input-context" {
                        // An older exact input had no host context. Recovery
                        // cannot upgrade it into newly asserted setup evidence.
                        prompt.injected_context.push(meerkat_core::ContentInput::Text("invented readiness".into()));
                    } else { prompt.content = meerkat_core::ContentInput::Text("other".into()); }
                    changed.bound_service_input = Some(serde_json::from_value(serde_json::to_value(input).unwrap()).unwrap());
                }
            }
            replace_row(&fixture, stored, changed).await;
            let refusal = fixture.source.restore_service_admission(&b).await;
            if corruption == "legacy-no-input" { assert!(matches!(refusal, Err(PrivateOriginalError::Unavailable))); }
            else { assert!(matches!(refusal, Err(PrivateOriginalError::Conflict))); }
            assert!(fixture.source.owner.historical_task_custody_for_test(record.final_association.as_ref().unwrap()).is_none());
            let stored = fixture.store.get(&fixture.source.key(&b).unwrap()).await.unwrap().unwrap();
            assert!(stored.data.body["service_admission"].is_null());
            assert_eq!(fixture.inputs().await.unwrap(), inputs);
            assert_eq!(fixture.model_requests(), requests);
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bound_input_persistence_failure_prevents_native_entry_and_cannot_be_replayed() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        fixture.source.bind_controller_pin_source(fixture.service.clone()).unwrap();
        let b = approved(&fixture).await;
        let (stored, _) = row(&fixture, &b).await;
        let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
        sql.execute_batch(&format!(
            "CREATE TRIGGER fail_bound_input BEFORE UPDATE OF data ON toolkit_records \
             WHEN NEW.id = '{}' AND json_extract(NEW.data, '$.body.bound_service_input') IS NOT NULL \
             BEGIN SELECT RAISE(ABORT, 'lost exact input evidence'); END;", stored.key.id.replace('\'', "''"),
        )).unwrap();
        let inputs = fixture.inputs().await.unwrap();
        let requests = fixture.model_requests();
        assert!(fixture.source.step_service_input(&stored).await.is_err());
        sql.execute_batch("DROP TRIGGER fail_bound_input;").unwrap();
        let (current, record) = row(&fixture, &b).await;
        assert!(record.final_association.is_some());
        assert!(record.bound_service_input.is_none());
        assert_eq!(fixture.source.owner.historical_task_custody_for_test(record.final_association.as_ref().unwrap()).unwrap().0, false);
        assert!(matches!(fixture.source.step_service_input(&current).await, Err(PrivateOriginalError::Unavailable)));
        assert_eq!(fixture.inputs().await.unwrap(), inputs);
        assert_eq!(fixture.model_requests(), requests);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_not_admitted_with_exact_bound_evidence_does_not_submit_again() {
    let fixture = ContinuationFixture::for_source_status(DeliveryMode::RefuseSecondBeforeLookup).await;
    let result = AssertUnwindSafe(Box::pin(async {
        fixture.source.bind_controller_pin_source(fixture.service.clone()).unwrap();
        let b = approved(&fixture).await;
        let (stored, _) = row(&fixture, &b).await;
        let inputs = fixture.inputs().await.unwrap();
        let requests = fixture.model_requests();
        assert!(fixture.source.step_service_input(&stored).await.is_err());
        let (current, record) = row(&fixture, &b).await;
        assert!(record.bound_service_input.is_some());
        forget_b(&fixture, &b, &record);
        assert!(fixture.source.restore_service_admission(&b).await.unwrap().is_none());
        assert!(matches!(fixture.source.step_service_input(&current).await, Err(PrivateOriginalError::Unavailable)));
        assert!(fixture.source.owner.historical_task_custody_for_test(record.final_association.as_ref().unwrap()).is_none());
        assert_eq!(fixture.inputs().await.unwrap(), inputs);
        assert_eq!(fixture.model_requests(), requests);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

/// Delay only the actual actor observation's return. This is an explicit
/// pre-B wait control; Core's retained #35 tests separately hold the real B.
struct PausedPins {
    inner: Arc<dyn ControllerPinSource>,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl ControllerPinSource for PausedPins {
    async fn live_actor(&self, session: &meerkat_core::types::SessionId)
        -> Option<meerkat::LiveSessionActorWitness>
    {
        let observed = self.inner.live_actor(session).await;
        self.entered.notify_one();
        self.release.notified().await;
        observed
    }
    async fn pin_actor(&self, _: &meerkat::LiveSessionActorWitness)
        -> Result<meerkat_core::ControllerModelClient, String>
    { panic!("cold observation must never acquire B through pin_actor"); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_wait_releases_source_writer_and_refuses_changed_or_busy_publication() {
    for mutation in ["cancel-parent", "unrelated-publication", "source-writer", "originals-lock"] {
        let fixture = ContinuationFixture::for_service_input().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let b = approved(&fixture).await;
            let (record, _) = admitted_without_ack(&fixture, &b).await;
            forget_b(&fixture, &b, &record);
            let pins = Arc::new(PausedPins { inner: fixture.service.clone(),
                entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new() });
            fixture.source.bind_controller_pin_source(pins.clone()).unwrap();
            let requests = fixture.model_requests();
            let inputs = fixture.inputs().await.unwrap();
            let observe = fixture.source.restore_service_admission(&b);
            tokio::pin!(observe);
            tokio::select! {
                _ = pins.entered.notified() => (),
                result = &mut observe => panic!("actor wait not reached: {}", result.is_ok()),
                _ = tokio::time::sleep(WAIT) => panic!("actor observation timed out"),
            }
            // Taking and releasing this writer while observation waits is the
            // discriminating assertion against the original across-await guard.
            drop(fixture.source.owner.try_source_writer().expect("source writer remains available"));
            let mut writer = None;
            let mut originals = None;
            if mutation == "cancel-parent" {
                fixture.source.record_task_group_cancelled(&record.approved_task.as_ref().unwrap().parent).await.unwrap();
            } else if mutation == "unrelated-publication" {
                drop(fixture.source.owner.publication().begin_owner_change().unwrap());
            } else if mutation == "source-writer" {
                writer = Some(fixture.source.owner.try_source_writer().unwrap());
            } else { originals = Some(fixture.source.owner.lock_originals_for_test()); }
            pins.release.notify_one();
            let refusal = tokio::time::timeout(WAIT, &mut observe).await.unwrap();
            assert!(matches!(refusal, Err(PrivateOriginalError::Unavailable | PrivateOriginalError::Denied)));
            drop(originals);
            drop(writer);
            assert!(fixture.source.owner.historical_task_custody_for_test(record.final_association.as_ref().unwrap())
                .is_none_or(|(admitted, _)| !admitted));
            let (_, current) = row(&fixture, &b).await;
            assert!(current.service_admission.is_none());
            assert_eq!(fixture.inputs().await.unwrap(), inputs);
            assert_eq!(fixture.model_requests(), requests);
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_native_b_wait_allows_source_cancellation_and_preserves_pending_refusal() {
    type PersistentFixture = ContinuationFixture<meerkat::PersistentSessionService<meerkat::FactoryAgentBuilder>>;
    for uncertain_write in [false, true] {
        let (fixture, _runtime_store) = PersistentFixture::for_live_attachment().await;
        let result = AssertUnwindSafe(Box::pin(async {
            let b = approved(&fixture).await;
            let (record, input_id) = admitted_without_ack(&fixture, &b).await;
            forget_b(&fixture, &b, &record);
            let pins = Arc::new(PausedPins { inner: fixture.service.clone(),
                entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new() });
            fixture.source.bind_controller_pin_source(pins.clone()).unwrap();
            let boundary = fixture.service.acquire_live_session_actor_turn_boundary_lease(&fixture.session)
                .await.unwrap();
            let observe = fixture.source.restore_service_admission(&b);
            tokio::pin!(observe);
            tokio::select! {
                _ = pins.entered.notified() => (),
                result = &mut observe => panic!("actor observation not reached: {}", result.is_ok()),
                _ = tokio::time::sleep(WAIT) => panic!("actor observation timed out"),
            }
            pins.release.notify_one();
            assert!(tokio::time::timeout(Duration::from_millis(20), &mut observe).await.is_err(),
                "native observer waits for the actual persistent actor B lease");
            let parent = &record.approved_task.as_ref().unwrap().parent;
            let sql = rusqlite::Connection::open(&fixture.source_path).unwrap();
            if uncertain_write {
                let parent_key = fixture.source.key(parent).unwrap();
                sql.execute_batch(&format!(
                    "CREATE TRIGGER fail_cold_parent_cancel BEFORE UPDATE OF data ON toolkit_records \
                     WHEN NEW.id = '{}' AND json_extract(NEW.data, '$.body.operation_cancelled') = 1 \
                     BEGIN SELECT RAISE(ABORT, 'uncertain parent cancellation'); END;",
                    parent_key.id.replace('\'', "''"),
                )).unwrap();
            }
            let cancellation = fixture.source.record_task_group_cancelled(parent).await;
            if uncertain_write { assert!(matches!(cancellation, Err(PrivateOriginalError::Unavailable))); }
            else { cancellation.unwrap(); }
            drop(boundary);
            assert!(matches!(tokio::time::timeout(WAIT, &mut observe).await.unwrap(),
                Err(PrivateOriginalError::Denied | PrivateOriginalError::Unavailable)));
            // A fresh read after the publication change still cannot treat the
            // existing Pending parent as open, even with no durable cancelled bit.
            if uncertain_write {
                pins.release.notify_one();
                assert!(matches!(tokio::time::timeout(WAIT, fixture.source.restore_service_admission(&b)).await.unwrap(),
                    Err(PrivateOriginalError::Unavailable)));
                sql.execute_batch("DROP TRIGGER fail_cold_parent_cancel;").unwrap();
                fixture.source.record_task_group_cancelled(parent).await.unwrap();
            }
            let (_, current) = row(&fixture, &b).await;
            assert!(current.service_admission.is_none());
            assert!(fixture.source.owner.historical_task_custody_for_test(record.final_association.as_ref().unwrap())
                .is_none_or(|(admitted, _)| !admitted));
            assert!(fixture.machine.input_state(&fixture.session, &input_id).await.unwrap().is_some(),
                "source cancellation preserves the historical native input");
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_source_does_not_keep_the_runtime_pin_owner_alive() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let b = approved(&fixture).await;
        let (record, _) = admitted_without_ack(&fixture, &b).await;
        forget_b(&fixture, &b, &record);
        let pins = Arc::new(PausedPins { inner: fixture.service.clone(),
            entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new() });
        let weak = Arc::downgrade(&pins);
        fixture.source.bind_controller_pin_source(pins.clone()).unwrap();
        drop(pins);
        assert!(weak.upgrade().is_none(), "source must not extend the exact runtime pin owner");
        let inputs = fixture.inputs().await.unwrap();
        assert!(matches!(fixture.source.restore_service_admission(&b).await, Err(PrivateOriginalError::Unavailable)));
        assert!(fixture.source.owner.historical_task_custody_for_test(record.final_association.as_ref().unwrap()).is_none());
        assert_eq!(fixture.inputs().await.unwrap(), inputs);
    })).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}
