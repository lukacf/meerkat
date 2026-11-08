use toolkit_catalog::*;

fn source() -> ToolkitSourceRecordMachineAuthority {
    let mut owner = ToolkitSourceRecordMachineAuthority::new();
    owner
        .apply(ToolkitSourceInput::Register {
            source_key: "source".into(),
            scope_digest: "scope".into(),
            source_kind: ToolkitSourceKind::Helper,
            helper_parent_report_ref: "parent-and-report".into(),
            preparation_id: "prepare-source".into(),
        })
        .unwrap();
    owner
}

fn seal_source(revision: u64) -> ToolkitSourceInput {
    ToolkitSourceInput::ConfirmPreparation {
        source_key: "source".into(),
        expected_revision: revision,
        preparation_id: "prepare-source".into(),
        preparation_receipt: "prepared-source".into(),
    }
}

fn prepared_offer() -> ToolkitOriginalOfferMachineAuthority {
    let mut owner = ToolkitOriginalOfferMachineAuthority::new();
    owner
        .apply(ToolkitOfferInput::Prepare {
            offer_key: "offer".into(),
            source_key: "source".into(),
            source_revision: 2,
            incarnation: "incarnation".into(),
            input_ref: "exact-bytes".into(),
            input_digest: "input-digest".into(),
            binding_digest: "all-bindings".into(),
            preparation_id: "prepare-offer".into(),
            preparation_epoch: 1,
        })
        .unwrap();
    owner
}

fn seal(owner: &mut ToolkitOriginalOfferMachineAuthority) {
    owner
        .apply(ToolkitOfferInput::SealPreparation {
            offer_key: "offer".into(),
            expected_revision: 1,
            preparation_id: "prepare-offer".into(),
            input_digest: "input-digest".into(),
            preparation_receipt: "prepared-offer".into(),
        })
        .unwrap();
}

fn offer(revision: u64, authority_current: bool) -> ToolkitOfferInput {
    ToolkitOfferInput::Offer {
        offer_key: "offer".into(),
        expected_revision: revision,
        source_key: "source".into(),
        source_revision: 2,
        current_incarnation: "incarnation".into(),
        source_open: true,
        source_ready: true,
        component_open: true,
        authority_current,
    }
}

fn sent_offer() -> ToolkitOriginalOfferMachineAuthority {
    let mut owner = prepared_offer();
    seal(&mut owner);
    let transition = owner.apply(offer(2, true)).unwrap();
    assert_eq!(
        transition.into_effects(),
        vec![
            ToolkitOfferEffect::SendIntent {
                offer_key: "offer".into(),
                input_ref: "exact-bytes".into(),
                input_digest: "input-digest".into(),
            },
            ToolkitOfferEffect::PublishOffer {
                offer_key: "offer".into(),
                revision: 3
            },
        ]
    );
    owner
}

fn start(owner: &mut ToolkitOriginalOfferMachineAuthority, epoch: u64) -> u64 {
    let dispatch_id = owner.state().dispatch_count + 1;
    owner
        .apply(ToolkitOfferInput::StartDispatch {
            offer_key: "offer".into(),
            expected_revision: owner.state().revision,
            epoch,
            dispatch_id,
        })
        .unwrap();
    dispatch_id
}

fn answer(
    owner: &ToolkitOriginalOfferMachineAuthority,
    dispatch_id: u64,
    answer: ToolkitDispatchAnswer,
    receipt: &str,
) -> ToolkitOfferInput {
    ToolkitOfferInput::ObserveDispatch {
        offer_key: "offer".into(),
        expected_revision: owner.state().revision,
        input_digest: "input-digest".into(),
        incarnation: "incarnation".into(),
        dispatch_id,
        answer,
        receipt_ref: receipt.into(),
    }
}

fn read(
    owner: &ToolkitOriginalOfferMachineAuthority,
    epoch: u64,
    observation: ToolkitCoreObservation,
    receipt: &str,
    kind: ToolkitCoreRead,
) -> ToolkitOfferInput {
    ToolkitOfferInput::ObserveRead {
        offer_key: "offer".into(),
        expected_revision: owner.state().revision,
        input_digest: "input-digest".into(),
        incarnation: "incarnation".into(),
        epoch,
        observation,
        receipt_ref: receipt.into(),
        read: kind,
    }
}

#[test]
fn source_preparation_rejects_stale_revision_without_mutation() {
    let mut owner = source();
    let before = owner.state().clone();
    assert!(owner.apply(seal_source(2)).is_err());
    assert_eq!(owner.state(), &before);
    owner.apply(seal_source(1)).unwrap();
    assert_eq!(owner.state().readiness, ToolkitSourceReadiness::Ready);
}

#[test]
fn cancellation_closes_source_before_effect_receipts_and_never_reopens() {
    let mut owner = source();
    owner
        .apply(ToolkitSourceInput::BeginCancellation {
            source_key: "source".into(),
            expected_revision: 1,
            cancellation_id: "cancel".into(),
        })
        .unwrap();
    assert_eq!(
        owner.state().source_phase,
        ToolkitSourcePhase::CancellationPending
    );
    assert_eq!(
        owner.state().readiness,
        ToolkitSourceReadiness::ClosedToOffers
    );
    assert!(owner.apply(seal_source(2)).is_err());
    let confirm = |published_revision| ToolkitSourceInput::ConfirmCancellation {
        source_key: "source".into(),
        expected_revision: 2,
        cancellation_id: "cancel".into(),
        published_revision,
        revocation_receipt: "revoked".into(),
        publication_receipt: "published".into(),
    };
    assert!(owner.apply(confirm(1)).is_err());
    owner.apply(confirm(2)).unwrap();
    assert_eq!(owner.state().source_phase, ToolkitSourcePhase::Cancelled);
    assert!(owner.apply(seal_source(3)).is_err());
}

#[test]
fn generated_prepared_authority_keeps_committed_owner_unchanged() {
    let mut owner = source();
    let mut ready = owner.prepare_authority();
    let mut cancelled = owner.prepare_authority();
    ready.apply(seal_source(1)).unwrap();
    cancelled
        .apply(ToolkitSourceInput::BeginCancellation {
            source_key: "source".into(),
            expected_revision: 1,
            cancellation_id: "cancel".into(),
        })
        .unwrap();
    assert_eq!(owner.state().revision, 1);
    assert_eq!(owner.state().readiness, ToolkitSourceReadiness::Preparing);
    owner.commit_prepared_authority(cancelled).unwrap();
    assert!(owner.commit_prepared_authority(ready).is_err());
    assert_eq!(
        owner.state().readiness,
        ToolkitSourceReadiness::ClosedToOffers
    );
}

#[test]
fn unsealed_offer_cannot_send_and_offered_work_cannot_withdraw_locally() {
    let mut prepared = prepared_offer();
    assert!(prepared.apply(offer(1, true)).is_err());
    let mut sent = sent_offer();
    assert!(sent.apply(offer(3, true)).is_err());
    assert!(
        sent.apply(ToolkitOfferInput::WithdrawPrepared {
            offer_key: "offer".into(),
            expected_revision: 3,
            epoch: 2,
        })
        .is_err()
    );
    assert_eq!(sent.state().offer_phase, ToolkitOfferPhase::Offered);
}

#[test]
fn a_failed_commit_check_withdraws_the_unsent_preparation() {
    let mut owner = prepared_offer();
    seal(&mut owner);
    owner.apply(offer(2, false)).unwrap();
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Withdrawn);
    assert_eq!(owner.state().dispatch_count, 0);
}

#[test]
fn a_withdrawal_records_its_cause_and_tells_a_verdict_apart() {
    let refused =
        |source_open, source_revision, incarnation: &str, component_open, authority_current| {
            let mut owner = prepared_offer();
            seal(&mut owner);
            owner
                .apply(ToolkitOfferInput::Offer {
                    offer_key: "offer".into(),
                    expected_revision: 2,
                    source_key: "source".into(),
                    source_revision,
                    current_incarnation: incarnation.into(),
                    source_open,
                    source_ready: true,
                    component_open,
                    authority_current,
                })
                .unwrap();
            assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Withdrawn);
            owner.state().withdrawal_cause
        };
    // Verdicts on the delivery.
    assert_eq!(
        refused(true, 2, "incarnation", true, false),
        ToolkitWithdrawalCause::PolicyDenied
    );
    assert_eq!(
        refused(false, 2, "incarnation", true, true),
        ToolkitWithdrawalCause::SourceClosed
    );
    assert_eq!(
        refused(true, 3, "incarnation", true, true),
        ToolkitWithdrawalCause::SourceClosed
    );
    // Not verdicts: the same delivery may be prepared again.
    assert_eq!(
        refused(true, 2, "incarnation", false, false),
        ToolkitWithdrawalCause::AdmissionClosed
    );
    assert_eq!(
        refused(true, 2, "replaced", true, true),
        ToolkitWithdrawalCause::PreparationStale
    );
    let mut owner = prepared_offer();
    owner
        .apply(ToolkitOfferInput::WithdrawPrepared {
            offer_key: "offer".into(),
            expected_revision: 1,
            epoch: 2,
        })
        .unwrap();
    assert_eq!(
        owner.state().withdrawal_cause,
        ToolkitWithdrawalCause::ProcessEnded
    );
    assert_eq!(
        sent_offer().state().withdrawal_cause,
        ToolkitWithdrawalCause::NotWithdrawn
    );
}

#[test]
fn only_an_ended_process_preparation_is_withdrawn_at_restart() {
    let mut owner = prepared_offer();
    let withdraw = |epoch| ToolkitOfferInput::WithdrawPrepared {
        offer_key: "offer".into(),
        expected_revision: 1,
        epoch,
    };
    assert!(owner.apply(withdraw(1)).is_err());
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Prepared);
    owner.apply(withdraw(2)).unwrap();
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Withdrawn);
}

#[test]
fn a_dispatch_is_recorded_with_its_epoch_and_the_next_id_before_its_call() {
    let mut owner = sent_offer();
    let stale = ToolkitOfferInput::StartDispatch {
        offer_key: "offer".into(),
        expected_revision: 3,
        epoch: 1,
        dispatch_id: 2,
    };
    assert!(owner.apply(stale).is_err());
    assert_eq!(start(&mut owner, 1), 1);
    assert_eq!(owner.state().dispatch_count, 1);
    assert_eq!(owner.state().dispatch_epochs.get(&1), Some(&1));
    assert_eq!(owner.state().last_dispatch_epoch, 1);
    assert_eq!(start(&mut owner, 2), 2);
    let earlier = ToolkitOfferInput::StartDispatch {
        offer_key: "offer".into(),
        expected_revision: owner.state().revision,
        epoch: 1,
        dispatch_id: 3,
    };
    assert!(owner.apply(earlier).is_err());
}

#[test]
fn a_lost_answer_is_recorded_against_its_dispatch_and_reads_that_change_nothing_are_no_inputs() {
    let mut owner = sent_offer();
    let first = start(&mut owner, 1);
    owner
        .apply(answer(&owner, first, ToolkitDispatchAnswer::Unresolved, ""))
        .unwrap();
    assert!(owner.state().unresolved_dispatches.contains(&first));
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Offered);
    let before = owner.state().clone();
    // The durable read never counts this process's lost dispatch: its write may still land.
    for observation in [
        ToolkitCoreObservation::Unavailable,
        ToolkitCoreObservation::Absent,
        ToolkitCoreObservation::Unresolved,
        ToolkitCoreObservation::NotAdmitted,
    ] {
        assert!(
            owner
                .apply(read(&owner, 1, observation, "", ToolkitCoreRead::Durable))
                .is_err()
        );
        assert_eq!(owner.state(), &before);
    }
    for observation in [
        ToolkitCoreObservation::Unavailable,
        ToolkitCoreObservation::Unresolved,
        ToolkitCoreObservation::NotAdmitted,
    ] {
        assert!(
            owner
                .apply(read(&owner, 1, observation, "", ToolkitCoreRead::Live))
                .is_err()
        );
        assert_eq!(owner.state(), &before);
    }
    // A later refusal cannot settle the record while the lost answer is unresolved.
    let second = start(&mut owner, 1);
    owner
        .apply(answer(
            &owner,
            second,
            ToolkitDispatchAnswer::CoreRefused,
            "core-refusal",
        ))
        .unwrap();
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Offered);
    assert!(!owner.state().dispatch_epochs.contains_key(&second));
}

#[test]
fn a_refusal_settles_only_the_one_open_dispatch_and_names_it() {
    for (kind, receipt_kind) in [
        (
            ToolkitDispatchAnswer::CoreRefused,
            ToolkitReceiptKind::CoreRefused,
        ),
        (
            ToolkitDispatchAnswer::EntryRefused,
            ToolkitReceiptKind::EntryRefused,
        ),
    ] {
        let mut owner = sent_offer();
        let first = start(&mut owner, 1);
        let second = start(&mut owner, 1);
        assert!(owner.apply(answer(&owner, second, kind, "")).is_ok());
        assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Offered);
        assert!(owner.apply(answer(&owner, first, kind, "")).is_err());
        owner
            .apply(answer(&owner, first, kind, "refusal-receipt"))
            .unwrap();
        assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::CoreObserved);
        assert_eq!(owner.state().receipt_kind, receipt_kind);
        assert_eq!(owner.state().receipt_dispatch, first);
        assert_eq!(owner.state().receipt_epoch, 1);
        assert_eq!(owner.state().core_receipt_ref, "refusal-receipt");
    }
}

#[test]
fn an_open_dispatch_of_an_ended_process_blocks_a_refusal() {
    let mut owner = sent_offer();
    start(&mut owner, 1);
    let redrive = start(&mut owner, 2);
    owner
        .apply(answer(
            &owner,
            redrive,
            ToolkitDispatchAnswer::EntryRefused,
            "fence",
        ))
        .unwrap();
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Offered);
    assert_eq!(owner.state().dispatch_epochs.len(), 1);
}

#[test]
fn an_admission_settles_with_its_dispatch_and_cannot_be_overwritten() {
    let mut owner = sent_offer();
    let first = start(&mut owner, 1);
    start(&mut owner, 1);
    assert!(
        owner
            .apply(answer(&owner, first, ToolkitDispatchAnswer::Admitted, ""))
            .is_err()
    );
    owner
        .apply(answer(
            &owner,
            first,
            ToolkitDispatchAnswer::Admitted,
            "immutable-core-receipt",
        ))
        .unwrap();
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::CoreObserved);
    assert_eq!(owner.state().receipt_kind, ToolkitReceiptKind::Admitted);
    assert_eq!(owner.state().receipt_dispatch, first);
    assert!(
        owner
            .apply(answer(
                &owner,
                2,
                ToolkitDispatchAnswer::Admitted,
                "different-receipt"
            ))
            .is_err()
    );
    assert!(
        owner
            .apply(read(
                &owner,
                1,
                ToolkitCoreObservation::Admitted,
                "different-receipt",
                ToolkitCoreRead::Live
            ))
            .is_err()
    );
}

#[test]
fn a_held_or_refused_bind_leaves_the_journal() {
    let mut owner = sent_offer();
    let first = start(&mut owner, 1);
    assert!(
        owner
            .apply(answer(
                &owner,
                first,
                ToolkitDispatchAnswer::NotSubmitted,
                "receipt"
            ))
            .is_err()
    );
    owner
        .apply(answer(
            &owner,
            first,
            ToolkitDispatchAnswer::NotSubmitted,
            "",
        ))
        .unwrap();
    assert!(owner.state().dispatch_epochs.is_empty());
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Offered);
}

#[test]
fn a_found_read_settles_without_naming_a_dispatch() {
    let mut owner = sent_offer();
    start(&mut owner, 1);
    owner
        .apply(read(
            &owner,
            2,
            ToolkitCoreObservation::Admitted,
            "found",
            ToolkitCoreRead::Durable,
        ))
        .unwrap();
    assert_eq!(owner.state().receipt_kind, ToolkitReceiptKind::Found);
    assert_eq!(owner.state().receipt_dispatch, 0);
    assert_eq!(owner.state().receipt_epoch, 0);
}

#[test]
fn final_absence_needs_this_process_dispatch_and_covers_the_dispatches_so_far() {
    let mut owner = sent_offer();
    let first = start(&mut owner, 1);
    owner
        .apply(answer(&owner, first, ToolkitDispatchAnswer::Unresolved, ""))
        .unwrap();
    // In the same process the durable read cannot speak for the lost answer: not final.
    assert!(
        owner
            .apply(read(
                &owner,
                1,
                ToolkitCoreObservation::Absent,
                "",
                ToolkitCoreRead::Durable
            ))
            .is_err()
    );
    // After a restart, before this process dispatched it (its re-drive owed): not final.
    for kind in [ToolkitCoreRead::Durable, ToolkitCoreRead::Live] {
        assert!(
            owner
                .apply(read(&owner, 2, ToolkitCoreObservation::Absent, "", kind))
                .is_err()
        );
    }
    let redrive = start(&mut owner, 2);
    // The re-drive in flight: not final.
    for kind in [ToolkitCoreRead::Durable, ToolkitCoreRead::Live] {
        assert!(
            owner
                .apply(read(&owner, 2, ToolkitCoreObservation::Absent, "", kind))
                .is_err()
        );
    }
    owner
        .apply(answer(
            &owner,
            redrive,
            ToolkitDispatchAnswer::NotSubmitted,
            "",
        ))
        .unwrap();
    // Every dispatch left open or lost is an ended process's: the durable read is final too.
    owner
        .apply(read(
            &owner,
            2,
            ToolkitCoreObservation::Absent,
            "",
            ToolkitCoreRead::Durable,
        ))
        .unwrap();
    assert_eq!(owner.state().absence_through, 2);
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Offered);
}

#[test]
fn a_live_read_counts_this_process_lost_dispatch_and_a_durable_read_never_does() {
    let mut owner = sent_offer();
    let first = start(&mut owner, 1);
    owner
        .apply(answer(&owner, first, ToolkitDispatchAnswer::Unresolved, ""))
        .unwrap();
    // Another dispatch of this process in flight blocks either read.
    let second = start(&mut owner, 1);
    for kind in [ToolkitCoreRead::Durable, ToolkitCoreRead::Live] {
        assert!(
            owner
                .apply(read(&owner, 1, ToolkitCoreObservation::Absent, "", kind))
                .is_err()
        );
    }
    owner
        .apply(answer(
            &owner,
            second,
            ToolkitDispatchAnswer::NotSubmitted,
            "",
        ))
        .unwrap();
    // Core's durable read takes no gate: the lost answer's write may land after it answers.
    let before = owner.state().clone();
    assert!(
        owner
            .apply(read(
                &owner,
                1,
                ToolkitCoreObservation::Absent,
                "",
                ToolkitCoreRead::Durable
            ))
            .is_err()
    );
    assert_eq!(owner.state(), &before);
    // Core's live read is ordered after any admission write in flight on the session: final
    // for this process's lost dispatch, and it covers every dispatch so far.
    owner
        .apply(read(
            &owner,
            1,
            ToolkitCoreObservation::Absent,
            "",
            ToolkitCoreRead::Live,
        ))
        .unwrap();
    assert_eq!(owner.state().absence_through, 2);
    assert!(owner.state().unresolved_dispatches.contains(&first));
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::Offered);
}

fn close(owner: &ToolkitOriginalOfferMachineAuthority, epoch: u64) -> ToolkitOfferInput {
    ToolkitOfferInput::CloseUnresolved {
        offer_key: "offer".into(),
        expected_revision: owner.state().revision,
        close_id: "close".into(),
        epoch,
    }
}

#[test]
fn the_close_records_the_unresolved_offer_and_no_dispatch_of_its_process_follows() {
    let mut owner = sent_offer();
    owner.apply(close(&owner, 1)).unwrap();
    assert_eq!(owner.state().close_epoch, 1);
    let after = ToolkitOfferInput::StartDispatch {
        offer_key: "offer".into(),
        expected_revision: owner.state().revision,
        epoch: 1,
        dispatch_id: 1,
    };
    assert!(owner.apply(after).is_err());
    // The same process's close records it once.
    assert!(owner.apply(close(&owner, 1)).is_err());
}

#[test]
fn a_later_process_dispatches_a_record_the_close_recorded() {
    let mut owner = sent_offer();
    let first = start(&mut owner, 1);
    owner
        .apply(answer(
            &owner,
            first,
            ToolkitDispatchAnswer::NotSubmitted,
            "",
        ))
        .unwrap();
    owner.apply(close(&owner, 1)).unwrap();
    // The next process's restart re-drives the record: its dispatch start is recorded.
    let redrive = start(&mut owner, 2);
    assert_eq!(redrive, 2);
    owner
        .apply(answer(
            &owner,
            redrive,
            ToolkitDispatchAnswer::Admitted,
            "core-receipt",
        ))
        .unwrap();
    assert_eq!(owner.state().offer_phase, ToolkitOfferPhase::CoreObserved);
    assert_eq!(owner.state().receipt_epoch, 2);
    assert_eq!(owner.state().close_epoch, 1);
}

#[test]
fn a_close_never_predates_the_dispatches_it_records() {
    let mut owner = sent_offer();
    start(&mut owner, 2);
    assert!(owner.apply(close(&owner, 1)).is_err());
    owner.apply(close(&owner, 2)).unwrap();
}

#[test]
fn recovery_requires_terminal_phase_and_receipt_to_agree() {
    let mut owner = sent_offer();
    let first = start(&mut owner, 1);
    owner
        .apply(answer(
            &owner,
            first,
            ToolkitDispatchAnswer::Admitted,
            "core-receipt",
        ))
        .unwrap();
    let observed = owner.state().clone();
    assert!(ToolkitOriginalOfferMachineAuthority::recover_from_state(observed.clone()).is_ok());

    let mut still_offered = observed.clone();
    still_offered.offer_phase = ToolkitOfferPhase::Offered;
    assert!(ToolkitOriginalOfferMachineAuthority::recover_from_state(still_offered).is_err());

    let mut missing_receipt = observed.clone();
    missing_receipt.core_receipt_ref.clear();
    assert!(ToolkitOriginalOfferMachineAuthority::recover_from_state(missing_receipt).is_err());

    let mut unnamed = observed.clone();
    unnamed.receipt_dispatch = 0;
    assert!(ToolkitOriginalOfferMachineAuthority::recover_from_state(unnamed).is_err());

    let mut unsealed = observed;
    unsealed.preparation_receipt.clear();
    assert!(ToolkitOriginalOfferMachineAuthority::recover_from_state(unsealed).is_err());
}

#[test]
fn original_catalog_schemas_validate_without_native_admission_phase() {
    source_record_schema().validate().unwrap();
    let schema = original_offer_schema();
    schema.validate().unwrap();
    assert!(
        schema
            .state
            .terminal_phases
            .iter()
            .any(|phase| phase.as_str() == "CoreObserved")
    );
    assert!(
        schema
            .state
            .phase
            .variants
            .iter()
            .all(|v| v.name.as_str() != "Admitted" && v.name.as_str() != "NotAdmitted")
    );
}

#[test]
fn recovery_rejects_absent_owners_with_invented_identity() {
    let mut source = ToolkitSourceRecordMachineAuthority::new().state().clone();
    source.source_key = "uncommitted-source".into();
    assert!(ToolkitSourceRecordMachineAuthority::recover_from_state(source).is_err());
    let mut offer = ToolkitOriginalOfferMachineAuthority::new().state().clone();
    offer.offer_key = "uncommitted-offer".into();
    assert!(ToolkitOriginalOfferMachineAuthority::recover_from_state(offer).is_err());
    let mut dispatched = ToolkitOriginalOfferMachineAuthority::new().state().clone();
    dispatched.dispatch_count = 1;
    assert!(ToolkitOriginalOfferMachineAuthority::recover_from_state(dispatched).is_err());
}
