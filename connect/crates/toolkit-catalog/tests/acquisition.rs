use toolkit_catalog::{
    AcquisitionEffect, AcquisitionInput, AcquisitionPhase, ToolkitAcquisitionMachineAuthority,
    ToolkitAcquisitionMachineMutator, ToolkitAcquisitionMachineState,
};

type Authority = ToolkitAcquisitionMachineAuthority;
type State = ToolkitAcquisitionMachineState;

fn admit() -> Authority {
    let mut authority = Authority::new();
    let transition = authority
        .apply(AcquisitionInput::Admit {
            operation_id: "operation-1".into(),
            request_digest: "request-1".into(),
            authority_digest: "authority-1".into(),
            continuation_id: "continuation-1".into(),
            expected_revision: 0,
            deadline: 100,
            now: 10,
        })
        .unwrap();
    assert_eq!(
        transition.into_effects(),
        vec![AcquisitionEffect::WorkAdmissionRequested {
            operation_id: "operation-1".into(),
            request_digest: "request-1".into(),
            authority_digest: "authority-1".into(),
        }]
    );
    authority
}

fn selection(state: &State, digest: &str, now: u64) -> AcquisitionInput {
    AcquisitionInput::RecordSelection {
        operation_id: state.operation_id.clone(),
        authority_digest: state.authority_digest.clone(),
        expected_revision: state.revision,
        selection_digest: digest.into(),
        decision_ref: format!("decision-{digest}"),
        now,
    }
}

fn preparing() -> Authority {
    let mut authority = admit();
    let transition = authority
        .apply(selection(authority.state(), "selection-a", 20))
        .unwrap();
    assert_eq!(
        transition.into_effects(),
        vec![AcquisitionEffect::SelectionRecorded {
            operation_id: "operation-1".into(),
            selection_digest: "selection-a".into(),
            selection_generation: 1,
            decision_ref: "decision-selection-a".into(),
        }]
    );
    authority
}

fn ready(state: &State, now: u64) -> AcquisitionInput {
    AcquisitionInput::ResolveReady {
        operation_id: state.operation_id.clone(),
        authority_digest: state.authority_digest.clone(),
        expected_revision: state.revision,
        selection_digest: state.selection_digest.clone(),
        selection_generation: state.selection_generation,
        readiness_ref: "verified-owner-receipts-1".into(),
        readiness_digest: "readiness-digest-1".into(),
        observed_at: now,
        now,
    }
}

fn cancel(state: &State, now: u64) -> AcquisitionInput {
    AcquisitionInput::Cancel {
        operation_id: state.operation_id.clone(),
        authority_digest: state.authority_digest.clone(),
        expected_revision: state.revision,
        detail_ref: "requester-cancellation-1".into(),
        now,
    }
}

fn expire(state: &State, now: u64) -> AcquisitionInput {
    AcquisitionInput::Expire {
        operation_id: state.operation_id.clone(),
        authority_digest: state.authority_digest.clone(),
        expected_revision: state.revision,
        now,
    }
}

fn acknowledge(state: &State, receipt: &str) -> AcquisitionInput {
    AcquisitionInput::AcknowledgeContinuation {
        operation_id: state.operation_id.clone(),
        authority_digest: state.authority_digest.clone(),
        expected_revision: state.revision,
        continuation_id: state.continuation_id.clone(),
        terminal_revision: state.terminal_revision,
        receipt_ref: receipt.into(),
    }
}

fn rejected_without_mutation(authority: &mut Authority, input: AcquisitionInput) {
    let prior = authority.state().clone();
    assert!(authority.apply(input).is_err());
    assert_eq!(authority.state(), &prior);
}

#[test]
fn admission_binds_identity_and_launches_work_once() {
    let mut authority = admit();
    assert_eq!(
        authority.state().lifecycle_phase,
        AcquisitionPhase::Investigating
    );
    assert_eq!(authority.state().revision, 1);
    assert_eq!(authority.state().deadline, 100);
    assert_eq!(authority.state().admitted_at, 10);
    rejected_without_mutation(
        &mut authority,
        AcquisitionInput::Admit {
            operation_id: "operation-2".into(),
            request_digest: "request-2".into(),
            authority_digest: "authority-2".into(),
            continuation_id: "continuation-2".into(),
            expected_revision: 1,
            deadline: 110,
            now: 20,
        },
    );
    for invalid in [
        AcquisitionInput::Admit {
            operation_id: String::new(),
            request_digest: "r".into(),
            authority_digest: "a".into(),
            continuation_id: "c".into(),
            expected_revision: 0,
            deadline: 20,
            now: 10,
        },
        AcquisitionInput::Admit {
            operation_id: "o".into(),
            request_digest: "r".into(),
            authority_digest: "a".into(),
            continuation_id: "c".into(),
            expected_revision: 1,
            deadline: 20,
            now: 10,
        },
        AcquisitionInput::Admit {
            operation_id: "o".into(),
            request_digest: "r".into(),
            authority_digest: "a".into(),
            continuation_id: "c".into(),
            expected_revision: 0,
            deadline: 10,
            now: 10,
        },
    ] {
        rejected_without_mutation(&mut Authority::new(), invalid);
    }
}

#[test]
fn cancellation_and_expiry_reject_late_success_and_selection() {
    for phase in [AcquisitionPhase::Cancelled, AcquisitionPhase::Expired] {
        let mut authority = preparing();
        let command = if phase == AcquisitionPhase::Cancelled {
            cancel(authority.state(), 30)
        } else {
            let early = expire(authority.state(), 99);
            rejected_without_mutation(&mut authority, early);
            expire(authority.state(), 100)
        };
        let effects = authority.apply(command).unwrap().into_effects();
        assert_eq!(
            effects,
            vec![AcquisitionEffect::ContinuationRequested {
                operation_id: "operation-1".into(),
                continuation_id: "continuation-1".into(),
                terminal_revision: 3,
                outcome: phase,
            }]
        );
        let late = ready(authority.state(), authority.state().last_observed_at);
        rejected_without_mutation(&mut authority, late);
        let late_selection = selection(authority.state(), "selection-b", 40);
        rejected_without_mutation(&mut authority, late_selection);
        let repeat = if phase == AcquisitionPhase::Cancelled {
            cancel(authority.state(), 40)
        } else {
            expire(authority.state(), 110)
        };
        rejected_without_mutation(&mut authority, repeat);
    }
}

#[test]
fn replaced_selection_requires_fresh_generation_even_after_aba() {
    let mut authority = preparing();
    let mut old_receipts = ready(authority.state(), 40);
    authority
        .apply(selection(authority.state(), "selection-b", 21))
        .unwrap();
    authority
        .apply(selection(authority.state(), "selection-a", 22))
        .unwrap();
    assert_eq!(authority.state().selection_generation, 3);
    if let AcquisitionInput::ResolveReady {
        expected_revision, ..
    } = &mut old_receipts
    {
        *expected_revision = authority.state().revision;
    }
    rejected_without_mutation(&mut authority, old_receipts);
    let repeated_selection = selection(authority.state(), "selection-a", 23);
    rejected_without_mutation(&mut authority, repeated_selection);
    let effects = authority
        .apply(ready(authority.state(), 40))
        .unwrap()
        .into_effects();
    assert_eq!(authority.state().lifecycle_phase, AcquisitionPhase::Ready);
    assert_eq!(
        effects,
        vec![AcquisitionEffect::ContinuationRequested {
            operation_id: "operation-1".into(),
            continuation_id: "continuation-1".into(),
            terminal_revision: 5,
            outcome: AcquisitionPhase::Ready,
        }]
    );
}

#[test]
fn readiness_rejects_wrong_identity_missing_evidence_and_stale_time() {
    let mut authority = preparing();
    for mutation in 0..11 {
        let mut command = ready(authority.state(), 30);
        if let AcquisitionInput::ResolveReady {
            operation_id,
            authority_digest,
            expected_revision,
            selection_digest,
            selection_generation,
            readiness_ref,
            readiness_digest,
            observed_at,
            now,
        } = &mut command
        {
            match mutation {
                0 => *operation_id = "other-operation".into(),
                1 => *authority_digest = "other-authority".into(),
                2 => *expected_revision -= 1,
                3 => *selection_digest = "other-selection".into(),
                4 => *selection_generation += 1,
                5 => readiness_ref.clear(),
                6 => readiness_digest.clear(),
                7 => *observed_at = 29,
                8 => *observed_at = 31,
                9 => {
                    *now = 100;
                    *observed_at = 100;
                }
                10 => {
                    *now = 19;
                    *observed_at = 19;
                }
                _ => unreachable!(),
            }
        }
        rejected_without_mutation(&mut authority, command);
    }
    authority.apply(ready(authority.state(), 30)).unwrap();
    assert_eq!(authority.state().readiness_ref, "verified-owner-receipts-1");
}

#[test]
fn terminal_outcomes_are_distinct_and_acknowledgment_is_bound_and_idempotent() {
    for outcome in [
        AcquisitionPhase::Ready,
        AcquisitionPhase::Unavailable,
        AcquisitionPhase::Failed,
        AcquisitionPhase::Cancelled,
        AcquisitionPhase::Expired,
    ] {
        let mut authority = preparing();
        let state = authority.state();
        let terminal = match outcome {
            AcquisitionPhase::Ready => ready(state, 30),
            AcquisitionPhase::Unavailable => AcquisitionInput::ReportUnavailable {
                operation_id: state.operation_id.clone(),
                authority_digest: state.authority_digest.clone(),
                expected_revision: state.revision,
                detail_ref: "unavailable-evidence".into(),
                now: 30,
            },
            AcquisitionPhase::Failed => AcquisitionInput::ReportFailure {
                operation_id: state.operation_id.clone(),
                authority_digest: state.authority_digest.clone(),
                expected_revision: state.revision,
                detail_ref: "failure-evidence".into(),
                now: 30,
            },
            AcquisitionPhase::Cancelled => cancel(state, 30),
            AcquisitionPhase::Expired => expire(state, 100),
            _ => unreachable!(),
        };
        let effects = authority.apply(terminal).unwrap().into_effects();
        assert_eq!(
            effects,
            vec![AcquisitionEffect::ContinuationRequested {
                operation_id: "operation-1".into(),
                continuation_id: "continuation-1".into(),
                terminal_revision: 3,
                outcome,
            }]
        );
        assert_eq!(authority.state().lifecycle_phase, outcome);
        for mutation in 0..6 {
            let mut command = acknowledge(authority.state(), "delivery-receipt-1");
            if let AcquisitionInput::AcknowledgeContinuation {
                operation_id,
                authority_digest,
                expected_revision,
                continuation_id,
                terminal_revision,
                receipt_ref,
            } = &mut command
            {
                match mutation {
                    0 => *operation_id = "other-operation".into(),
                    1 => *authority_digest = "other-authority".into(),
                    2 => *expected_revision -= 1,
                    3 => *continuation_id = "other-continuation".into(),
                    4 => *terminal_revision -= 1,
                    5 => receipt_ref.clear(),
                    _ => unreachable!(),
                }
            }
            rejected_without_mutation(&mut authority, command);
        }
        let ack_effects = authority
            .apply(acknowledge(authority.state(), "delivery-receipt-1"))
            .unwrap()
            .into_effects();
        assert_eq!(
            ack_effects,
            vec![AcquisitionEffect::ContinuationAcknowledged {
                operation_id: "operation-1".into(),
                continuation_id: "continuation-1".into(),
                terminal_revision: 3,
                receipt_ref: "delivery-receipt-1".into(),
            }]
        );
        let prior = authority.state().clone();
        let repeated = authority
            .apply(acknowledge(authority.state(), "delivery-receipt-1"))
            .unwrap()
            .into_effects();
        assert!(repeated.is_empty());
        assert_eq!(authority.state(), &prior);
        assert_eq!(authority.state().lifecycle_phase, outcome);
        let changed_receipt = acknowledge(authority.state(), "different-receipt");
        rejected_without_mutation(&mut authority, changed_receipt);
        let late = ready(authority.state(), 40);
        rejected_without_mutation(&mut authority, late);
    }
}

#[test]
fn every_mutation_rejects_stale_revision_and_changed_authority() {
    for mutation in 0..3 {
        for command_index in 0..6 {
            let mut authority = preparing();
            let state = authority.state();
            let mut command = match command_index {
                0 => selection(state, "selection-b", 30),
                1 => ready(state, 30),
                2 => AcquisitionInput::ReportUnavailable {
                    operation_id: state.operation_id.clone(),
                    authority_digest: state.authority_digest.clone(),
                    expected_revision: state.revision,
                    detail_ref: "evidence".into(),
                    now: 30,
                },
                3 => AcquisitionInput::ReportFailure {
                    operation_id: state.operation_id.clone(),
                    authority_digest: state.authority_digest.clone(),
                    expected_revision: state.revision,
                    detail_ref: "evidence".into(),
                    now: 30,
                },
                4 => cancel(state, 30),
                5 => expire(state, 100),
                _ => unreachable!(),
            };
            match &mut command {
                AcquisitionInput::RecordSelection {
                    operation_id,
                    authority_digest,
                    expected_revision,
                    ..
                }
                | AcquisitionInput::ResolveReady {
                    operation_id,
                    authority_digest,
                    expected_revision,
                    ..
                }
                | AcquisitionInput::ReportUnavailable {
                    operation_id,
                    authority_digest,
                    expected_revision,
                    ..
                }
                | AcquisitionInput::ReportFailure {
                    operation_id,
                    authority_digest,
                    expected_revision,
                    ..
                }
                | AcquisitionInput::Cancel {
                    operation_id,
                    authority_digest,
                    expected_revision,
                    ..
                }
                | AcquisitionInput::Expire {
                    operation_id,
                    authority_digest,
                    expected_revision,
                    ..
                } => match mutation {
                    0 => *operation_id = "other-operation".into(),
                    1 => *authority_digest = "other-authority".into(),
                    2 => *expected_revision -= 1,
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            }
            rejected_without_mutation(&mut authority, command);
        }
    }
}

#[test]
fn revision_capacity_reserves_terminal_resolution_and_acknowledgment() {
    let mut state = preparing().state().clone();
    state.revision = u64::MAX - 2;
    state.selection_generation = u64::MAX - 3;
    let mut authority = Authority::recover_from_state(state).unwrap();
    let extra_selection = selection(authority.state(), "selection-b", 30);
    rejected_without_mutation(&mut authority, extra_selection);
    authority.apply(cancel(authority.state(), 30)).unwrap();
    assert_eq!(authority.state().revision, u64::MAX - 1);
    authority
        .apply(acknowledge(authority.state(), "last-receipt"))
        .unwrap();
    assert_eq!(authority.state().revision, u64::MAX);
    let effects = authority
        .apply(acknowledge(authority.state(), "last-receipt"))
        .unwrap()
        .into_effects();
    assert!(effects.is_empty());
    let impossible = cancel(authority.state(), 40);
    rejected_without_mutation(&mut authority, impossible);
}

#[test]
fn release_recovery_rejects_invalid_states_without_debug_assertions() {
    let preparing = preparing().state().clone();
    for mutation in 0..14 {
        let mut state = preparing.clone();
        match mutation {
            0 => state.lifecycle_phase = AcquisitionPhase::Absent,
            1 => state.operation_id.clear(),
            2 => state.request_digest.clear(),
            3 => state.authority_digest.clear(),
            4 => state.continuation_id.clear(),
            5 => state.deadline = state.admitted_at,
            6 => state.last_observed_at = state.admitted_at - 1,
            7 => state.selection_digest.clear(),
            8 => state.decision_ref.clear(),
            9 => state.selection_generation = 0,
            10 => state.revision = u64::MAX,
            11 => state.readiness_ref = "phantom-readiness".into(),
            12 => state.terminal_revision = state.revision,
            13 => state.continuation_receipt_ref = "phantom-delivery".into(),
            _ => unreachable!(),
        }
        assert!(
            Authority::recover_from_state(state).is_err(),
            "mutation {mutation}"
        );
    }
    let mut authority = Authority::recover_from_state(preparing).unwrap();
    authority.apply(ready(authority.state(), 30)).unwrap();
    for mutation in 0..8 {
        let mut state = authority.state().clone();
        match mutation {
            0 => state.readiness_ref.clear(),
            1 => state.readiness_digest.clear(),
            2 => state.readiness_observed_at -= 1,
            3 => state.terminal_revision = 0,
            4 => state.terminal_at = 100,
            5 => state.revision += 1,
            6 => state.detail_ref = "wrong-outcome-detail".into(),
            7 => state.lifecycle_phase = AcquisitionPhase::Expired,
            _ => unreachable!(),
        }
        assert!(
            Authority::recover_from_state(state).is_err(),
            "ready mutation {mutation}"
        );
    }
}

#[test]
fn persisted_state_roundtrips_but_deserialization_is_not_recovery() {
    let mut authority = preparing();
    for step in 0..3 {
        if step == 1 {
            authority.apply(ready(authority.state(), 30)).unwrap();
        } else if step == 2 {
            authority
                .apply(acknowledge(authority.state(), "receipt"))
                .unwrap();
        }
        let bytes = serde_json::to_vec(authority.state()).unwrap();
        let state: State = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(&state, authority.state());
        assert_eq!(
            Authority::recover_from_state(state).unwrap().state(),
            authority.state()
        );
    }
    let mut wire = serde_json::to_value(authority.state()).unwrap();
    wire["operation_id"] = serde_json::Value::String(String::new());
    let state: State = serde_json::from_value(wire.clone()).unwrap();
    assert!(Authority::recover_from_state(state).is_err());
    wire["unexpected_field"] = serde_json::Value::Bool(true);
    assert!(serde_json::from_value::<State>(wire).is_err());
    assert!(serde_json::from_str::<AcquisitionPhase>("\"mystery\"").is_err());
}

#[test]
fn prepared_transition_waits_for_persistence_and_rejects_changed_base() {
    let mut authority = preparing();
    let mut first = authority.prepare_authority();
    let mut stale = authority.prepare_authority();
    first.apply(cancel(authority.state(), 30)).unwrap();
    stale.apply(ready(authority.state(), 30)).unwrap();
    assert_eq!(
        authority.state().lifecycle_phase,
        AcquisitionPhase::Preparing
    );
    authority.commit_prepared_authority(first).unwrap();
    assert!(authority.commit_prepared_authority(stale).is_err());
    assert_eq!(
        authority.state().lifecycle_phase,
        AcquisitionPhase::Cancelled
    );
}

#[test]
fn catalog_schema_and_models_come_from_the_same_generated_authority() {
    let schema = toolkit_catalog::acquisition_schema();
    schema.validate().unwrap();
    assert_eq!(schema.machine.as_str(), "ToolkitAcquisitionMachine");
    assert_eq!(schema.rust.crate_name, "meerkat_toolkit");
    assert_eq!(schema.rust.module, "setup");
    let mut transition_schema = schema.clone();
    transition_schema.named_types.clear();
    assert_eq!(&transition_schema, State::schema_static());
    let (model, ci, deep) = toolkit_catalog::acquisition_model_artifacts().unwrap();
    let next = model.split_once("\nNext ==\n").unwrap().1;
    let next = next.split("\n\n").next().unwrap();
    for transition in &schema.transitions {
        let action = format!(" {}(", transition.name);
        let disjunct = next.lines().find(|line| line.contains(&action)).unwrap();
        assert!(
            disjunct.contains("\\E expected_revision \\in {revision} : "),
            "{} must remain reachable after revision 2",
            transition.name,
        );
        if matches!(
            &transition.on,
            meerkat_machine_schema::TriggerMatch::Input { variant, .. }
                if variant.as_str() == "AcknowledgeContinuation"
        ) {
            assert!(
                disjunct
                    .contains("\\E arg_terminal_revision \\in (0..2 \\cup {3, 4, 5, 6, 7, 8}) : "),
                "{} must explore terminal revisions beyond the default domain",
                transition.name,
            );
        }
    }
    for invariant in &schema.invariants {
        assert!(model.contains(invariant.name.as_str()));
        assert!(ci.contains(invariant.name.as_str()));
        assert!(deep.contains(invariant.name.as_str()));
    }
}

#[test]
fn strict_coverage_attributes_every_schema_element_explicitly() {
    let coverage = toolkit_catalog::coverage::acquisition_coverage();
    let admission = coverage
        .transition_coverage
        .iter()
        .find(|entry| entry.name == "AdmitRequest")
        .unwrap();
    assert_eq!(admission.anchor_ids, ["admission-owner"]);
    assert_eq!(
        admission.scenario_ids,
        ["admission_binds_identity_and_launches_work_once"]
    );
    let continuation = coverage
        .effect_coverage
        .iter()
        .find(|entry| entry.name == "ContinuationRequested")
        .unwrap();
    assert_eq!(continuation.anchor_ids, ["resolution-owner"]);
    meerkat_machine_schema::catalog::validate_machine_coverage(
        &toolkit_catalog::acquisition_schema(),
        &coverage,
        meerkat_machine_schema::catalog::CoverageValidationMode::RequireClaims,
    )
    .unwrap();
}
