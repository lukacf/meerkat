//! Actual native receipts and the selected source task share the close fence.
//! These controls do not enable installed-pair cold restoration or manufacture
//! native outcomes. The existing bridge pauses or poisons only after acceptance.
use super::*;
use crate::original_ingress::custody_attempt::CustodyDecision;
use crate::private_originals::custody_recovery::HandoffPhase;
use meerkat_runtime::input_state::verify_exact_replay;

#[derive(Clone, Copy)]
enum CloseOrder { Acknowledged, PublicationFailed, BeforeCallback, PoisonedBeforeCallback }

fn pending(fixture: &Fixture) -> Arc<PendingOriginalAdmission> {
    let held = fixture.source.retained.lock().unwrap().iter()
        .find(|held| held.reference == fixture.submission.original)
        .cloned().expect("the actual bound original remains owned");
    let pending = held.first_custody.lock().unwrap().clone()
        .expect("the actual first invocation is enrolled");
    assert!(pending.handoff.enrolled());
    assert!(pending.first_candidate);
    pending
}

fn publication(pending: &PendingOriginalAdmission) -> Option<(bool, Result<(), String>)> {
    match &*pending.pending.lock().unwrap() {
        SourcePublication::Pending(_) => None,
        SourcePublication::Settled { source_acknowledged, result } =>
            Some((*source_acknowledged, result.clone())),
    }
}

async fn exercise(order: CloseOrder) {
    let mode = match order {
        CloseOrder::Acknowledged => DeliveryMode::Normal,
        CloseOrder::PublicationFailed => DeliveryMode::PoisonPublicationAfterAcceptance,
        CloseOrder::BeforeCallback | CloseOrder::PoisonedBeforeCallback => DeliveryMode::HoldAccepted,
    };
    let fixture = Fixture::new(mode, false).await;
    // The three existing fixture phases have a finite observation bound. This
    // does not impose a wall-clock guarantee on the source owner's actual join.
    // Retain one actual shutdown future across the body observer. Reusing its
    // completed result avoids starting native shutdown again during cleanup.
    let mut native_shutdown = Box::pin(fixture.shutdown());
    let mut native_cleanup = None;
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(WAIT * 3, async {
        fixture.source.start_custody_recovery(
            fixture.native.mob_runtime().continuation_source_announcer(),
        ).unwrap();
        let result = if matches!(order, CloseOrder::BeforeCallback | CloseOrder::PoisonedBeforeCallback) {
            let submit = fixture.submit();
            tokio::pin!(submit);
            tokio::select! {
                _ = fixture.fault.entered.notified() => {},
                result = &mut submit => panic!("the actual native reply was not held: {}",
                    submission_context(&result)),
            }
            assert_eq!(fixture.fault.accepted.load(Ordering::SeqCst), 1);
            let held = pending(&fixture);
            assert!(held.native_admission.lock().unwrap().is_none(),
                "Meta has not yet delivered its actual acknowledgement");
            assert!(publication(&held).is_none());
            assert_eq!(fixture.observe_contributor().await, Ok(false));
            let claims = fixture.association.candidate();
            assert!(matches!(fixture.owner.retained_custody(&claims.ingress_namespace, &claims.original_work),
                Ok(meerkat_runtime::meerkat_machine::RetainedCustody::AdmissionPending)));
            assert!(matches!(meerkat_runtime::meerkat_machine::require_source_original_live(
                fixture.owner.grants().as_ref(), fixture.owner.as_ref(), &claims.ingress_namespace,
                &claims.original_work, &claims.logical_executor, claims.represented_subject.as_ref()),
                Err(meerkat_runtime::input_authority::NativeAdmissionError::Readiness(
                    meerkat_core::authorization::ControllerReadinessFailure::OriginalAdmissionPending))));
            // A source read has no I/O/native lock dependency: hold the actual
            // source writer and the handoff lock and require prompt refusal.
            {
                let _writer = fixture.owner.source_writer().await;
                held.handoff.assert_read_is_nonblocking_for_test(&fixture.owner, claims);
            }
            if matches!(order, CloseOrder::PoisonedBeforeCallback) {
                // Poison only the actual source control while the real native
                // acceptance reply is held. This does not fabricate a receipt.
                fixture.source.poison_custody_control_for_test();
            } else {
                fixture.source.stop_custody_recovery();
            }
            assert!(!matches!(fixture.owner.retained_custody(&claims.ingress_namespace, &claims.original_work),
                Ok(meerkat_runtime::meerkat_machine::RetainedCustody::AdmissionPending)),
                "stop fences pending before the actual native callback returns");
            fixture.fault.release.add_permits(1);
            submit.await
        } else {
            let result = fixture.submit().await;
            // No await separates observing the actual callback result from
            // close. Acknowledgement already won, or its real error is retained.
            fixture.source.stop_custody_recovery();
            result
        };
        let held = pending(&fixture);
        let before_close_publication = publication(&held);
        let Ok(PrivateOriginalSubmission::Admitted {
            session_id, input_id, deduplicated: false, turn_ticket: Some(ticket),
            source_settlement,
        }) = &result else {
            panic!("actual admitted receipt must remain admitted: {}", submission_context(&result))
        };
        assert_eq!(session_id, &fixture.session.to_string());
        assert_eq!(input_id, &held.input.id().to_string());
        assert!(TurnTicket::parse(ticket).is_ok());
        match order {
            CloseOrder::Acknowledged => {
                assert!(matches!(source_settlement, PrivateSourceSettlement::FreshAcceptancePromoted));
                assert_eq!(before_close_publication, Some((true, Ok(()))));
                assert_eq!(fixture.observe_contributor().await, Ok(true));
            }
            CloseOrder::PublicationFailed => {
                assert!(matches!(source_settlement, PrivateSourceSettlement::PostAdmissionPublicationFailed));
                assert!(matches!(&before_close_publication, Some((false, Err(_)))));
                assert_eq!(fixture.observe_contributor().await, Ok(false));
            }
            CloseOrder::BeforeCallback | CloseOrder::PoisonedBeforeCallback => {
                assert!(matches!(source_settlement, PrivateSourceSettlement::PostAdmissionPublicationFailed));
                assert!(!matches!(&before_close_publication, Some((true, _))),
                    "close or poisoned control won before callback, so live source publication is fenced");
                assert_eq!(fixture.observe_contributor().await, Ok(false));
            }
        }
        let native_receipt = held.native_admission.lock().unwrap().clone()
            .expect("retain the actual matching native receipt despite source failure");
        assert_eq!(native_receipt.session_id, fixture.session);
        assert_eq!(native_receipt.input_id, *held.input.id());
        assert!(!native_receipt.deduplicated);
        let native_row = fixture.machine.input_state(&held.session, held.input.id()).await
            .unwrap().expect("the exact native input was admitted");
        assert_eq!(native_row.state.input_id, *held.input.id());
        verify_exact_replay(&native_row.state, &held.input).unwrap();
        assert_eq!(fixture.native_inputs().await, Ok((1, 1)));
        let key = fixture.source.key(&fixture.submission.original).unwrap();
        let row = fixture.source.store.get(&key).await.unwrap().unwrap();
        let first = fixture.source.decode_record(&row, &fixture.submission.original).unwrap()
            .custody_attempt.expect("canonical first-attempt evidence");
        assert!(first.matches_input(&held.input).unwrap());

        // Exercise the real host ordering: source close, native shutdown, then
        // source join. Publication recovery must not depend on a live actor now.
        native_cleanup = Some(native_shutdown.as_mut().await);
        native_cleanup.as_ref().unwrap().as_ref().unwrap();
        fixture.source.join_custody_recovery().await.unwrap();
        assert!(held.handoff.phase() == HandoffPhase::Done);
        let settled = publication(&held).expect("joined terminal source guard");
        match order {
            CloseOrder::Acknowledged => assert_eq!(settled, (true, Ok(()))),
            CloseOrder::PublicationFailed => assert_eq!(Some(settled), before_close_publication,
                "join must preserve the original publication error and false acknowledgement"),
            CloseOrder::BeforeCallback | CloseOrder::PoisonedBeforeCallback => assert!(matches!(settled, (false, Err(_))),
                "durable promotion after close must not manufacture live acknowledgement"),
        }
        let row = fixture.source.store.get(&key).await.unwrap().unwrap();
        let terminal = fixture.source.decode_record(&row, &fixture.submission.original).unwrap()
            .custody_attempt.unwrap();
        assert!(terminal.same_attempt(&first));
        assert!(terminal.matches_input(&held.input).unwrap());
        assert!(terminal.decision() == CustodyDecision::Promoted);
        assert!(!terminal.observation_entered());
        let retained = held.native_admission.lock().unwrap().clone().unwrap();
        assert_eq!(retained.session_id, native_receipt.session_id);
        assert_eq!(retained.input_id, native_receipt.input_id);
        assert_eq!(retained.deduplicated, native_receipt.deduplicated);
        assert_eq!(fixture.fault.accepted.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.fault.refused.load(Ordering::SeqCst), 0);
        fixture.source.join_custody_recovery().await.unwrap();
    })).catch_unwind().await;

    // Retain both actual owners after assertion, timeout or observer loss.
    // Cleanup is not hidden behind the body timeout or a dropped join future.
    fixture.source.stop_custody_recovery();
    fixture.fault.release.add_permits(1);
    let native_cleanup = match native_cleanup {
        Some(completed) => completed,
        None => native_shutdown.as_mut().await,
    };
    let source_cleanup = fixture.source.join_custody_recovery().await;
    match checked {
        Ok(Ok(())) => {
            assert!(native_cleanup.is_ok(), "native cleanup failed: {native_cleanup:?}");
            assert!(source_cleanup.is_ok(), "source cleanup failed: {source_cleanup:?}");
        }
        Ok(Err(_)) => panic!("receipt/close control timed out; native cleanup: {native_cleanup:?}; source cleanup: {source_cleanup:?}"),
        Err(payload) => {
            if native_cleanup.is_err() || source_cleanup.is_err() {
                eprintln!("receipt/close cleanup failed: native={native_cleanup:?}; source={source_cleanup:?}");
            }
            std::panic::resume_unwind(payload);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledged_native_receipt_wins_before_close_and_remains_promoted() {
    exercise(CloseOrder::Acknowledged).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_publication_failure_joins_after_native_shutdown_without_live_acknowledgement() {
    exercise(CloseOrder::PublicationFailed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_before_actual_accepted_callback_retains_receipt_without_live_publication() {
    exercise(CloseOrder::BeforeCallback).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poisoned_source_control_before_actual_callback_preserves_receipt_without_publication() {
    exercise(CloseOrder::PoisonedBeforeCallback).await;
}
