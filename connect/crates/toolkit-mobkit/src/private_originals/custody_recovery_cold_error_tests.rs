//! Actual installed native evidence survives a completed cold terminal-write
//! failure. The wrapper never commits that terminal write; no restored source,
//! native refusal, delivered wake or successful cold publication is claimed.
use super::*;
use crate::original_ingress::custody_attempt::CustodyChange;
use meerkat_runtime::input_state::InputLifecycleState;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cold_native_observation_survives_failed_terminal_write_and_cached_close() {
    let (fixture, history, retained, continuation, store) = open_installed().await;
    let mut active = Some(fixture);
    let mut history = Some(history);
    let mut retained = Some(retained);
    let mut continuation = Some(continuation);
    let mut store = Some(store);
    let mut native_close = Some(active.as_ref().unwrap().retained_close_for_test());
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 8, Box::pin(async {
        let (association, input, native_record, first_row, grants, session) = {
            let fixture = active.as_ref().unwrap();
            let store = store.as_ref().unwrap();
            assert!(fixture.source.owner.has_installed_pair_recovery().unwrap());
            store.terminal_mode.store(1, Ordering::SeqCst);
            fixture.source.start_custody_recovery(
                fixture.continuation_native().continuation_source_announcer()).unwrap();
            let admitted = fixture.submit().await;
            assert!(matches!(&admitted, Ok(PrivateOriginalSubmission::Admitted {
                deduplicated: false, source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
            })), "the first input must actually admit before any cold observation");
            fixture.finish(&admitted).await.unwrap();
            let context = fixture.probe.as_ref().unwrap().captured();
            tokio::time::timeout(WAIT, fixture.machine.wait_run_settled(
                &fixture.session, context.retained_work().unwrap().run_id())).await.unwrap();
            let association = fixture.binding().await.unwrap().unwrap();
            let held = fixture.source.retained.lock().unwrap().iter()
                .find(|held| held.reference == *fixture.original_reference()).cloned().unwrap();
            let pending = held.first_custody.lock().unwrap().clone().unwrap();
            assert!(pending.handoff.enrolled());
            let receipt = pending.native_admission.lock().unwrap().clone().unwrap();
            assert_eq!(receipt.session_id, fixture.session);
            assert_eq!(&receipt.input_id, pending.input.id());
            assert!(!receipt.deduplicated);
            fixture.source.stop_custody_recovery();
            let (native, source) = tokio::join!(native_close.as_mut().unwrap().join(),
                fixture.source.join_custody_recovery());
            assert!(native.is_ok(), "first real native/local close failed: {native:?}");
            assert!(matches!(source, Err(crate::private_originals::PrivateOriginalError::Unavailable)));
            assert!(pending.handoff.closed_failure());
            assert!(store.terminal_calls.load(Ordering::SeqCst) > 0);
            assert_eq!(store.terminal_returns.load(Ordering::SeqCst), store.terminal_calls.load(Ordering::SeqCst));
            let row = fixture.cold_source_record().await;
            let attempt: CustodyAttempt = serde_json::from_value(row.data.body["custody_attempt"].clone()).unwrap();
            assert!(attempt.decision() == CustodyDecision::Pending);
            assert!(!attempt.observation_entered());
            attempt.validate_binding(&association).unwrap();
            let rows = history.as_ref().unwrap().load_input_states_strict(
                &LogicalRuntimeId::for_session(&fixture.session)).await.unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].seed.phase, InputLifecycleState::Consumed);
            assert!(rows[0].state.persisted_input.is_none(), "completed native payload was retired");
            let input: meerkat_runtime::Input = serde_json::from_value(
                row.data.body["bound_service_input"].clone()).unwrap();
            assert!(input.header().ingress_context.is_none(), "canonical bytes retain no process owner");
            assert_eq!(rows[0].state.input_id, receipt.input_id);
            assert!(attempt.matches_input(&input).unwrap());
            verify_exact_replay(&rows[0].state, &input).unwrap();
            let grants = std::fs::read(fixture.source_path.parent().unwrap().join("grants.jsonl")).unwrap();
            (association, input, serde_json::to_value(&rows[0]).unwrap(), row, grants, fixture.session.clone())
        };

        // No first-process source, grant owner, native machine, store facade or
        // retained close future may bridge the real installed-pair reopening.
        let weak_source = Arc::downgrade(&active.as_ref().unwrap().source);
        let weak_owner = Arc::downgrade(&active.as_ref().unwrap().source.owner);
        let weak_machine = Arc::downgrade(&active.as_ref().unwrap().machine);
        let weak_service = Arc::downgrade(&active.as_ref().unwrap().service);
        let weak_lifetime = Arc::downgrade(active.as_ref().unwrap().lifetime());
        let weak_history = Arc::downgrade(history.as_ref().unwrap());
        let weak_continuation = Arc::downgrade(continuation.as_ref().unwrap());
        let weak_store = Arc::downgrade(store.as_ref().unwrap());
        drop(native_close.take());
        drop(continuation.take());
        drop(store.take());
        drop(active.take());
        drop(history.take());
        assert!(weak_source.upgrade().is_none());
        assert!(weak_owner.upgrade().is_none());
        assert!(weak_machine.upgrade().is_none());
        assert!(weak_service.upgrade().is_none());
        assert!(weak_lifetime.upgrade().is_none());
        assert!(weak_history.upgrade().is_none());
        assert!(weak_continuation.upgrade().is_none());
        assert!(weak_store.upgrade().is_none());

        let (fixture, reopened_history, reopened_continuation, reopened_store) =
            reopen_installed(retained.take().unwrap()).await;
        native_close = Some(fixture.retained_close_for_test());
        active = Some(fixture);
        history = Some(reopened_history);
        continuation = Some(reopened_continuation);
        store = Some(reopened_store);
        let fixture = active.as_ref().unwrap();
        let store = store.as_ref().unwrap();
        assert_eq!(fixture.session, session);
        assert_eq!(fixture.model_requests(), 0);
        assert!(fixture.cold_source_custody(&association).is_none());
        assert!(fixture.source.owner.has_installed_pair_recovery().unwrap());
        assert!(!fixture.source.owner.has_live_first_binding(&first_row.key).unwrap());
        assert_eq!(fixture.cold_source_record().await, first_row);
        store.terminal_mode.store(2, Ordering::SeqCst);
        fixture.source.start_custody_recovery(
            fixture.continuation_native().continuation_source_announcer()).unwrap();
        // This is the actual cold owner's terminal CAS after reading the
        // persisted native input. The wrapper retains that entered future.
        store.terminal_entered.notified().await;
        assert_eq!(store.terminal_calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.terminal_returns.load(Ordering::SeqCst), 0);
        assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
        let charged_row = store.inner.get(&first_row.key).await.unwrap().unwrap();
        let first: CustodyAttempt = serde_json::from_value(first_row.data.body["custody_attempt"].clone()).unwrap();
        let charged: CustodyAttempt = serde_json::from_value(charged_row.data.body["custody_attempt"].clone()).unwrap();
        assert!(charged == first.changed(CustodyChange::ChargeResolution).unwrap());
        assert!(charged.decision() == CustodyDecision::Pending && charged.observation_entered());
        assert_eq!(charged_row.revision, first_row.revision + 1);
        let mut expected = first_row.data.body.clone();
        expected["custody_attempt"] = charged_row.data.body["custody_attempt"].clone();
        assert_eq!(charged_row.data.body, expected);
        fixture.source.stop_custody_recovery();
        let mut joining = Box::pin(fixture.source.join_custody_recovery());
        assert!(matches!(futures::poll!(joining.as_mut()), std::task::Poll::Pending));
        drop(joining);
        assert_eq!(store.terminal_returns.load(Ordering::SeqCst), 0);
        assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
        store.terminal_release.add_permits(1);
        let (native, source) = tokio::join!(native_close.as_mut().unwrap().join(),
            fixture.source.join_custody_recovery());
        assert!(native.is_ok(), "reopened native/local owners must join: {native:?}");
        assert!(matches!(source, Err(crate::private_originals::PrivateOriginalError::Unavailable)));
        assert_eq!(store.terminal_returns.load(Ordering::SeqCst), 1);
        let failed = fixture.source.retained_cold_failures_for_test().await;
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, first_row.key);
        let observed = failed[0].1.as_ref().expect("actual completed Present observation remains in the joined task");
        assert_eq!(&observed.state.input_id, input.id());
        assert_eq!(observed.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(serde_json::to_value(observed).unwrap(), native_record,
            "the failed task retains the complete actual native observation");
        verify_exact_replay(&observed.state, &input).unwrap();
        assert!(fixture.cold_source_custody(&association).is_none());
        assert_eq!(store.inner.get(&first_row.key).await.unwrap().unwrap(), charged_row);
        assert_eq!(std::fs::read(fixture.source_path.parent().unwrap().join("grants.jsonl")).unwrap(), grants);
        let counters = (store.reads.load(Ordering::SeqCst), store.terminal_calls.load(Ordering::SeqCst),
            store.terminal_returns.load(Ordering::SeqCst));
        for _ in 0..2 {
            assert!(matches!(fixture.source.join_custody_recovery().await,
                Err(crate::private_originals::PrivateOriginalError::Unavailable)));
            assert_eq!((store.reads.load(Ordering::SeqCst), store.terminal_calls.load(Ordering::SeqCst),
                store.terminal_returns.load(Ordering::SeqCst)), counters);
            let retained = fixture.source.retained_cold_failures_for_test().await;
            assert_eq!(retained.len(), 1);
            assert_eq!(retained[0].0, first_row.key);
            verify_exact_replay(&retained[0].1.as_ref().unwrap().state, &input).unwrap();
        }
        let rows = history.as_ref().unwrap().load_input_states_strict(&LogicalRuntimeId::for_session(&session)).await.unwrap();
        assert_eq!(rows.len(), 1, "cold recovery cannot submit another input");
        verify_exact_replay(&rows[0].state, &input).unwrap();
        assert_eq!(fixture.model_requests(), 0);
        assert_eq!(store.cancelled.load(Ordering::SeqCst), 0);
    }))).catch_unwind().await;
    if let Some(store) = store.as_ref() {
        store.release.add_permits(1);
        store.terminal_release.add_permits(1);
    }
    let cleanup = if let Some(fixture) = active.as_ref() {
        fixture.source.stop_custody_recovery();
        let (native, source) = tokio::join!(native_close.as_mut().unwrap().join(),
            fixture.source.join_custody_recovery());
        (native, source)
    } else { (Ok(()), Ok(())) };
    drop(native_close);
    drop(continuation);
    drop(store);
    drop(active);
    drop(history);
    match checked {
        Err(payload) => {
            if cleanup.0.is_err() { eprintln!("cold-error native cleanup failed: {:?}", cleanup.0); }
            std::panic::resume_unwind(payload);
        }
        Ok(result) => {
            assert!(cleanup.0.is_ok(), "actual native cleanup failed: {:?}", cleanup.0);
            assert!(matches!(cleanup.1, Err(crate::private_originals::PrivateOriginalError::Unavailable)));
            assert!(result.is_ok(), "actual cold failed-write observation timed out");
        }
    }
}
