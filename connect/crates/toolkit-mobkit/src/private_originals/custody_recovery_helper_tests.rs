//! Actual helper receipt survives a poisoned installed publication. Returned
//! store failures join as Unavailable, preserving the uncertain canonical row.
//! This does not claim promotion, native refusal, or a successful helper turn.
//! The real recovery task begins only after the admitted helper poison boundary.
//! Concurrent foreground admission and recovery are outside this close control.
use super::*;
use crate::original_ingress::custody_attempt::{CustodyAttempt, CustodyDecision};
use crate::private_originals::custody_recovery::HandoffPhase;
use crate::private_originals::governed_submit::SourcePublication;
use meerkat_runtime::input_state::verify_exact_replay;

fn publication(pending: &setup_helpers::PendingHelperAdmission) -> Option<(bool, Result<(), String>)> {
    match &pending.publication.lock().unwrap().source {
        SourcePublication::Pending(_) => None,
        SourcePublication::Settled { source_acknowledged, result } =>
            Some((*source_acknowledged, result.clone())),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_native_receipt_poisoned_publication_joins_unavailable_without_retry() {
    let script = acquisition_denial::Script::paused_for_helper_entry();
    let fault = Arc::new(DeliveryFault::new(DeliveryMode::PoisonPublicationAfterSecondAcceptance));
    let (fixture, history) = Fixture::for_helper_entry(script.clone(), fault.clone()).await;
    let mut first_round = Some(fixture.source.hold_first_custody_round_for_test());
    let mut native_shutdown = Box::pin(fixture.shutdown());
    let mut native_cleanup = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 3, async {
        // Select the real source task before either first input. The helper's
        // actual binder must enroll its v5 attempt, not an observer afterward.
        fixture.source.start_custody_recovery(
            fixture.native.mob_runtime().continuation_source_announcer(),
        ).unwrap();
        acquisition_approved::select_reviewed_setup(&fixture).await;
        let first = fixture.submit().await;
        assert!(matches!(first, Ok(PrivateOriginalSubmission::Admitted {
            deduplicated: false, source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
        })), "{}", submission_context(&first));
        script.first_response_waiting().await;
        script.release_first_response();
        let (held, handed) = loop {
            let held = {
                let helpers = fixture.source.helper_sources.lock().unwrap();
                assert!(helpers.len() <= 1);
                helpers.first().cloned()
            };
            if let Some(held) = held {
                let mut handoffs = held.handoffs_for_test();
                if !handoffs.is_empty() {
                    assert_eq!(handoffs.len(), 1);
                    let handed = handoffs.pop().unwrap().expect("actual typed helper handoff");
                    break (held, handed);
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        // Fence live publication before any further observation can await.
        fixture.source.stop_custody_recovery();
        let pending = held.first_custody.lock().unwrap().clone()
            .expect("first helper invocation is retained by its real source");
        assert!(pending.handoff.enrolled());
        assert!(handed.first_candidate);
        assert_eq!(handed.source_acknowledged, Some(false));
        let before_publication = publication(&pending);
        assert!(matches!(&before_publication, Some((false, Err(_)))));
        assert_eq!(pending.session, handed.session);
        assert_eq!(serde_json::to_value(&pending.input).unwrap(), handed.input);
        let actual = handed.native_admission.as_ref().expect("matching native callback receipt");
        assert!(!actual.deduplicated);
        assert_eq!(actual.session_id, pending.session);
        assert_eq!(&actual.input_id, pending.input.id());
        assert_eq!(pending.input.header().authority_association.as_ref(), Some(held.original().association()));
        assert_eq!(fault.accepted.load(Ordering::SeqCst), 2,
            "one actual Original and one actual helper fresh admission");

        let rows = acquisition_approved::native_history(history.as_ref(), &pending.session).await;
        assert_eq!(rows.len(), 1, "publication failure must not resubmit helper input");
        assert_eq!(rows[0].state.input_id, actual.input_id);
        assert_eq!(rows[0].state.authority_contributors.len(), 1);
        assert_eq!(rows[0].state.authority_contributors[0].association(), held.original().association());
        verify_exact_replay(&rows[0].state, &pending.input).unwrap();
        let key = held.source_record().key.clone();
        let before = fixture.store.get(&key).await.unwrap().unwrap();
        assert_eq!(before.data.version, 5);
        let first_attempt: CustodyAttempt = serde_json::from_value(
            before.data.body["custody_attempt"].clone(),
        ).unwrap();
        first_attempt.validate_binding(held.original().association()).unwrap();
        assert!(first_attempt.matches_input(&pending.input).unwrap());

        // Preserve the actual host order: close source admission, join native
        // shutdown, then join the same source task after actors are gone.
        assert_eq!(pending.handoff.write_attempts.load(Ordering::SeqCst), 0,
            "the actual recovery task has not entered its first round");
        first_round.take().unwrap().send(()).unwrap();
        native_cleanup = Some(native_shutdown.as_mut().await);
        native_cleanup.as_ref().unwrap().as_ref().unwrap();
        assert!(matches!(fixture.source.join_custody_recovery().await,
            Err(crate::private_originals::PrivateOriginalError::Unavailable)));
        assert!(pending.handoff.phase() == HandoffPhase::Accepted);
        assert!(pending.handoff.closed_failure());
        let writes = pending.handoff.write_attempts.load(Ordering::SeqCst);
        assert!(writes > 0, "the actual installed CAS returned a publication error");
        assert_eq!(publication(&pending), before_publication,
            "failed close preserves the actual publication error and false acknowledgement");
        let after = fixture.store.get(&key).await.unwrap().unwrap();
        assert_eq!(after, before, "failed publication cannot manufacture a terminal row");
        assert!(first_attempt.decision() == CustodyDecision::Pending);
        let retained = pending.publication.lock().unwrap().native_admission.clone().unwrap();
        assert_eq!(retained.session_id, actual.session_id);
        assert_eq!(retained.input_id, actual.input_id);
        assert_eq!(retained.deduplicated, actual.deduplicated);
        assert_eq!(fault.accepted.load(Ordering::SeqCst), 2);
        assert_eq!(fault.refused.load(Ordering::SeqCst), 0);
        let settled_rows = acquisition_approved::native_history(history.as_ref(), &pending.session).await;
        assert_eq!(settled_rows.len(), 1);
        assert_eq!(settled_rows[0].state.input_id, actual.input_id);
        verify_exact_replay(&settled_rows[0].state, &pending.input).unwrap();
        assert!(matches!(fixture.source.join_custody_recovery().await,
            Err(crate::private_originals::PrivateOriginalError::Unavailable)));
        assert_eq!(pending.handoff.write_attempts.load(Ordering::SeqCst), writes,
            "the same completed failed join must not start another CAS");
        assert_eq!(fixture.store.get(&key).await.unwrap().unwrap(), after,
            "same-owner rejoin preserves the uncertain canonical evidence");
        assert!(fixture.source.start_custody_recovery(None).is_err());
    })).catch_unwind().await;

    // Failure or a cancelled body observer retains both real cleanup owners.
    // The cleanup itself is not wrapped in the observation timeout.
    fixture.source.stop_custody_recovery();
    drop(first_round.take());
    script.release_first_response();
    let native_cleanup = match native_cleanup {
        Some(completed) => completed,
        None => native_shutdown.as_mut().await,
    };
    let source_cleanup = fixture.source.join_custody_recovery().await;
    match checked {
        Ok(Ok(())) => {
            assert!(native_cleanup.is_ok(), "native cleanup failed: {native_cleanup:?}");
            assert!(matches!(source_cleanup, Err(crate::private_originals::PrivateOriginalError::Unavailable)),
                "source cleanup must retain its real failure: {source_cleanup:?}");
        }
        Ok(Err(_)) => panic!("helper receipt/close timed out; native={native_cleanup:?}; source={source_cleanup:?}"),
        Err(payload) => {
            if native_cleanup.is_err() || source_cleanup.is_err() {
                eprintln!("helper receipt/close cleanup failed: native={native_cleanup:?}; source={source_cleanup:?}");
            }
            std::panic::resume_unwind(payload);
        }
    }
}
