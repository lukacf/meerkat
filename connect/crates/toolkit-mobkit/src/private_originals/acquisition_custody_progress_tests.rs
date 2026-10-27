//! The real registered acquisition invocation is retained after its actual
//! inner admission while the same source worker commits custody bookkeeping.
//! This exercises the recheck boundary, not historical failure attribution.
use super::*;
use crate::original_ingress::custody_attempt::{CustodyAttempt, CustodyChange, CustodyDecision};
use crate::private_originals::custody_recovery::HandoffPhase;
use crate::ToolAdmissionError;
use meerkat_runtime::input_state::verify_exact_replay;
use toolkit_store::{StoredRecord, Transition};

type Fixture = FirstFixture<meerkat::PersistentSessionService<FactoryAgentBuilder>>;

#[derive(Clone, Copy)]
enum Change { CustodyOnly, Cancel, SetupSelection }

// Pure typed comparison checks over this fixture's real exact first attempt.
// Derived rows are not written and make no native admission claim.
fn accounting_checks(source: &PrivateOriginals, reference: &PrivateOriginalRef, prior: &StoredRecord) {
    let record = source.decode_record(prior, reference).unwrap();
    let first = record.custody_attempt.clone().unwrap();
    let mut next = first.changed(CustodyChange::ChargeResolution).unwrap()
        .changed(CustodyChange::ResolutionSettled).unwrap()
        .changed(CustodyChange::ChargeResolution).unwrap();
    let derived = |attempt: CustodyAttempt| {
        let mut typed = record.clone();
        typed.custody_attempt = Some(attempt);
        let data = crate::private_originals::encode(typed).unwrap();
        StoredRecord { key: prior.key.clone(), revision: prior.revision + 3,
            content_digest: data.digest().unwrap(), data, updated_at_ms: prior.updated_at_ms }
    };
    let advanced = derived(next.clone());
    assert!(source.same_acquisition_source(reference, prior, &advanced).unwrap());
    let mut jumped = advanced.clone();
    jumped.revision = prior.revision + 1;
    assert!(!source.same_acquisition_source(reference, prior, &jumped).unwrap());
    assert!(!source.same_acquisition_source(reference, &advanced, prior).unwrap());
    next = next.changed(CustodyChange::Promote).unwrap();
    let mut terminal = derived(next);
    terminal.revision = prior.revision + 4;
    assert!(source.same_acquisition_source(reference, prior, &terminal).unwrap());
    assert!(source.same_acquisition_source(reference, &terminal, &terminal).unwrap());
    let withdrawn = derived(first.changed(CustodyChange::Withdraw).unwrap());
    assert!(!source.same_acquisition_source(reference, prior, &withdrawn).unwrap());
    let mut different = advanced.clone();
    different.data.body["custody_attempt"]["input_id"] =
        serde_json::to_value(meerkat_core::lifecycle::InputId::new()).unwrap();
    different.content_digest = different.data.digest().unwrap();
    assert!(!matches!(source.same_acquisition_source(reference, prior, &different), Ok(true)));
}

async fn exercise(change: Change) {
    let script = Script::paused_for_helper_entry();
    let (fixture, history) = Fixture::for_approved_acquisition(script.clone()).await;
    let installed = script.installed_owner();
    let pause = installed.hold_admission_recheck_for_test();
    let mut first_round = Some(fixture.source.hold_first_custody_round_for_test());
    let mut native_close = Box::pin(fixture.shutdown());
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 3, async {
        fixture.source.start_custody_recovery(
            fixture.native.mob_runtime().continuation_source_announcer()).unwrap();
        acquisition_approved::select_reviewed_setup(&fixture).await;
        let first = fixture.submit().await;
        assert!(matches!(&first, Ok(PrivateOriginalSubmission::Admitted {
            deduplicated: false, source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
        })), "{}", submission_context(&first));
        script.first_response_waiting().await;
        let held = fixture.source.retained.lock().unwrap().iter()
            .find(|held| held.reference == *fixture.original_reference()).cloned().unwrap();
        let pending = held.first_custody.lock().unwrap().clone().unwrap();
        let receipt = pending.native_admission.lock().unwrap().clone().unwrap();
        assert!(pending.handoff.phase() == HandoffPhase::Accepted);
        let key = fixture.source.key(fixture.original_reference()).unwrap();
        let before = fixture.store.get(&key).await.unwrap().unwrap();
        let before_typed = fixture.source.decode_record(&before, fixture.original_reference()).unwrap();
        let attempt = before_typed.custody_attempt.clone().unwrap();
        assert!(attempt.decision() == CustodyDecision::Pending);
        assert!(attempt.matches_input(&pending.input).unwrap());
        accounting_checks(&fixture.source, fixture.original_reference(), &before);

        script.release_first_response();
        pause.entered().await;
        // The real inner host has admitted this actual native invocation.
        // No source mutation has occurred since its first canonical lookup.
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), before);
        drop(first_round.take());
        let promoted = loop {
            let row = fixture.store.get(&key).await.unwrap().unwrap();
            let typed = fixture.source.decode_record(&row, fixture.original_reference()).unwrap();
            if typed.custody_attempt.as_ref().unwrap().decision() == CustodyDecision::Promoted {
                break row;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(promoted.revision, before.revision + 1);
        assert_ne!(promoted.content_digest, before.content_digest,
            "the previous whole-row comparison rejects this actual progress");
        let mut promoted_typed = fixture.source.decode_record(&promoted, fixture.original_reference()).unwrap();
        let terminal = promoted_typed.custody_attempt.replace(attempt.clone()).unwrap();
        assert!(terminal.same_attempt(&attempt));
        assert!(promoted_typed == before_typed, "all non-custody fields remain exact");
        assert!(fixture.source.same_acquisition_source(fixture.original_reference(), &before, &promoted).unwrap());

        match change {
            Change::CustodyOnly => {}
            Change::Cancel => fixture.source.record_operation_cancelled(fixture.original_reference()).await.unwrap(),
            Change::SetupSelection => {
                // A valid-checksum, typed substantive source change. Keep the
                // actual association/native receipt exact while changing the
                // reviewed setup's saved event reference. The second lookup
                // still decodes the unchanged canonical event and binding.
                let _writer = fixture.source.owner.source_writer().await;
                let current = fixture.store.get(&key).await.unwrap().unwrap();
                let mut typed = fixture.source.decode_record(&current, fixture.original_reference()).unwrap();
                let selection = typed.setup_request.as_mut().unwrap();
                let changed_event = crate::private_originals::digest(b"different reviewed setup event");
                assert_ne!(selection.original.event_sha256, changed_event);
                selection.original.event_sha256 = changed_event;
                let changed = fixture.store.compare_and_swap(Transition {
                    key: key.clone(), expected_revision: current.revision,
                    expected_digest: Some(current.content_digest), claim: None,
                    data: crate::private_originals::encode(typed).unwrap(), events: vec![], effects: vec![],
                }).await.unwrap();
                fixture.source.decode_record(&changed, fixture.original_reference()).unwrap();
                assert!(!fixture.source.same_acquisition_source(fixture.original_reference(), &before, &changed).unwrap());
            }
        }
        pause.release();
        let actual = pause.outcome().await;
        match change {
            Change::CustodyOnly => {
                assert!(actual.is_ok(), "actual wrapper refused custody-only progress: {actual:?}");
                fixture.finish(&first).await.unwrap();
            }
            Change::Cancel | Change::SetupSelection => {
                assert!(matches!(actual, Err(ToolAdmissionError::Denied)), "{actual:?}");
                assert!(fixture.store.list_records(&acquisition_approved::scope(&fixture), None, 10)
                    .await.unwrap().is_empty(), "refused admission must not create setup work");
            }
        }
        let rows = acquisition_approved::native_history(history.as_ref(), &fixture.session).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state.input_id, receipt.input_id);
        verify_exact_replay(&rows[0].state, &pending.input).unwrap();
    })).catch_unwind().await;

    // These owners live outside the cancellable body. Release both retained
    // futures before joining native and source custody on every exit.
    drop(first_round.take());
    pause.release();
    script.release_first_response();
    fixture.source.stop_custody_recovery();
    let native = native_close.as_mut().await;
    let source = fixture.source.join_custody_recovery().await;
    match checked {
        Ok(Ok(())) => { assert!(native.is_ok(), "{native:?}"); assert!(source.is_ok(), "{source:?}"); }
        Ok(Err(_)) => panic!("admission recheck timed out; native={native:?}; source={source:?}"),
        Err(payload) => {
            if native.is_err() || source.is_err() { eprintln!("admission recheck cleanup: {native:?}; {source:?}"); }
            std::panic::resume_unwind(payload);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_admission_allows_actual_custody_progress_during_inner_await() {
    exercise(Change::CustodyOnly).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_admission_refuses_cancellation_during_inner_await() {
    exercise(Change::Cancel).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_admission_refuses_substantive_source_change_during_inner_await() {
    exercise(Change::SetupSelection).await;
}
