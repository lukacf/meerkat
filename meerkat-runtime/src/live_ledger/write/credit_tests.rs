use super::*;
use crate::live_ledger::authority::dsl::{
    LiveEffectPhase, LiveRequestInput as Input, LiveRequestMachineAuthority as Authority,
    LiveRequestMachineMutator as Mutator,
};
use crate::live_ledger::authority::store::effect_credits::{
    EffectCompletionBudget, completion_digest, effect_phase, reserved_completion_charge,
};
use crate::live_ledger::completion::{
    LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES, LiveCompletionEvent, LiveCompletionRecord,
    LiveCompletionText, LivePhysicalEffectOutcome,
};
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_observation::LiveObservationSeq;
use meerkat_core::ops::OperationId;

fn claimed() -> TestResult<(Authority, OperationId, OperationId)> {
    let request = OperationId::new();
    let claim = OperationId::new();
    let mut owner = Authority::new();
    for mut input in generated_request_setup() {
        match &mut input {
            Input::Reserve { request_id, .. }
            | Input::Admit { request_id, .. }
            | Input::Stage { request_id, .. } => *request_id = request.to_string(),
            _ => {}
        }
        Mutator::apply(&mut owner, input)?;
    }
    let mut input = generated_effect_claim();
    let Input::ClaimEffect {
        request_id,
        claim_id,
        ..
    } = &mut input
    else {
        return Err("expected claim".into());
    };
    *request_id = request.to_string();
    *claim_id = claim.to_string();
    Mutator::apply(&mut owner, input)?;
    Ok((owner, request, claim))
}

pub(super) fn record(
    session_id: &SessionId,
    request: OperationId,
    claim: OperationId,
    sequence: u64,
    outcome: LivePhysicalEffectOutcome,
) -> TestResult<LiveCompletionRecord> {
    Ok(LiveCompletionRecord {
        format: LiveLedgerFormatV1::V1,
        session_id: session_id.clone(),
        channel_id: LiveChannelId::new("\0".repeat(128)),
        sequence: LiveObservationSeq::new(sequence)?,
        event: LiveCompletionEvent::EffectTerminal {
            claim_id: claim,
            request_id: request,
            outcome,
            token_accounting:
                meerkat_core::execution_scope::ScopedEffectTokenAccounting::NotApplicable {},
            diagnostic: LiveCompletionText::new("\0".repeat(LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES))?,
        },
    })
}

pub(super) fn settlement(record: &LiveCompletionRecord) -> TestResult<Input> {
    let LiveCompletionEvent::EffectTerminal {
        claim_id,
        request_id,
        outcome,
        token_accounting,
        ..
    } = &record.event
    else {
        return Err("expected effect completion".into());
    };
    let charge = record.encode()?.charge();
    Ok(Input::SettleEffect {
        claim_id: claim_id.to_string(),
        request_id: request_id.to_string(),
        run_id: "run".into(),
        target: "target-and-arguments".into(),
        outcome: effect_phase(*outcome),
        completion_records: charge.records,
        completion_bytes: charge.encoded_bytes,
        completion_sequence: record.sequence.get(),
        completion_digest: completion_digest(record)?,
        local_noninvocation_proven: *outcome == LivePhysicalEffectOutcome::NotStarted,
        token_accounting_status: token_accounting.status(),
        token_accounting_record: serde_json::to_string(token_accounting)?,
        observed_tokens: token_accounting.known_tokens().unwrap_or(0),
    })
}

#[test]
fn outstanding_effect_credits_reserve_future_head_revision_capacity() -> TestResult {
    let session_id = SessionId::new();
    let (owner, _, _) = claimed()?;
    let initial = PreparedLiveLedgerCommit::from_request_transition(
        &session_id,
        None,
        &owner.prepare_authority(),
    )?;
    let mut before = initial.successor;
    let reserved = before.payload.reserved.records;
    assert!(reserved >= 2);
    before.reference.revision = u64::try_from(i64::MAX)? - reserved - 1;
    let last_safe = PreparedLiveLedgerCommit::from_request_transition(
        &session_id,
        Some(&before),
        &owner.prepare_authority(),
    )?;
    assert_eq!(
        last_safe.successor.reference.revision + reserved,
        u64::try_from(i64::MAX)?
    );
    assert!(
        PreparedLiveLedgerCommit::from_request_transition(
            &session_id,
            Some(&last_safe.successor),
            &owner.prepare_authority(),
        )
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn full_capacity_request_settlement_funds_actual_record_and_snapshot_growth() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let request = OperationId::new();
        let input_id = meerkat_core::lifecycle::InputId::new();
        let run_id = meerkat_core::lifecycle::RunId::new();
        let mut owner = Authority::new();
        for mut input in generated_request_setup() {
            match &mut input {
                Input::Reserve { request_id, .. } => *request_id = request.to_string(),
                Input::Admit {
                    request_id,
                    input_id: admitted,
                    ..
                } => {
                    *request_id = request.to_string();
                    *admitted = input_id.to_string();
                }
                Input::Stage {
                    request_id,
                    input_id: staged,
                    run_id: run,
                    ..
                } => {
                    *request_id = request.to_string();
                    *staged = input_id.to_string();
                    *run = run_id.to_string();
                }
                _ => {}
            }
            Mutator::apply(&mut owner, input)?;
        }
        let mut initial = PreparedLiveLedgerCommit::from_request_transition(
            fixture.session.id(),
            None,
            &owner.prepare_authority(),
        )?;
        let quota = initial
            .successor
            .payload
            .used
            .checked_add(initial.successor.payload.reserved)?;
        initial.quota = quota;
        fixture
            .ops()?
            .commit_live_ledger(initial, current_fence())
            .await?;
        let before = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let completion = LiveCompletionRecord {
            format: LiveLedgerFormatV1::V1,
            session_id: fixture.session.id().clone(),
            channel_id: LiveChannelId::new("\0".repeat(128)),
            sequence: LiveObservationSeq::new(1)?,
            event: LiveCompletionEvent::RequestOutcome {
                request_id: request.clone(),
                outcome: crate::live_ledger::completion::LiveRequestCompletionFact::Completed {
                    input_id: input_id.clone(),
                    run_id: run_id.clone(),
                    result: LiveCompletionText::new(
                        "\0".repeat(crate::live_ledger::completion::LIVE_RESULT_MAX_BYTES),
                    )?,
                },
            },
        };
        let charge = completion.encode()?.charge();
        Mutator::apply(
            &mut owner,
            Input::Complete {
                request_id: request.to_string(),
                input_id: input_id.to_string(),
                run_id: run_id.to_string(),
                ordinary_completion_digest: "f".repeat(64),
                completion_records: charge.records,
                completion_bytes: charge.encoded_bytes,
                completion_sequence: completion.sequence.get(),
                completion_digest:
                    crate::live_ledger::authority::store::request_credits::completion_digest(
                        &completion,
                    )?,
            },
        )?;
        let mut commit = PreparedLiveLedgerCommit::from_request_transition_with_completions(
            fixture.session.id(),
            Some(&before),
            &owner.prepare_authority(),
            vec![completion],
        )?;
        commit.quota = quota;
        assert_eq!(
            commit.successor.payload.reserved,
            crate::live_ledger::authority::store::cancellation::reserved_charge(owner.state())?
                .checked_add(LiveResourceCharge {
                    records: 0,
                    encoded_bytes: 20
                })?
        );
        assert!(commit.successor.payload.used.fits_within(quota));
        assert_eq!(
            commit.successor.payload.used.records,
            before.payload.used.records + 1
        );
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(commit, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Committed { .. }
        ));
    }
    Ok(())
}

#[tokio::test]
async fn full_capacity_effect_settlement_funds_actual_records_and_snapshot_growth() -> TestResult {
    for backend in backends() {
        for outcome in LivePhysicalEffectOutcome::ALL {
            let fixture = Fixture::new(backend).await?;
            let (mut owner, request, claim) = claimed()?;
            let request_reservation =
                crate::live_ledger::authority::store::request_credits::reserved_completion_charge(
                    owner.state(),
                )?
                .checked_add(
                    crate::live_ledger::authority::store::cancellation::reserved_charge(
                        owner.state(),
                    )?,
                )?
                .checked_add(
                    PreparedLiveLedgerCommit::initial_head(fixture.session.id())?
                        .payload
                        .reserved,
                )?;
            let mut initial = PreparedLiveLedgerCommit::from_request_transition(
                fixture.session.id(),
                None,
                &owner.prepare_authority(),
            )?;
            let quota = initial
                .successor
                .payload
                .used
                .checked_add(initial.successor.payload.reserved)?;
            initial.quota = quota;
            assert!(matches!(
                fixture
                    .ops()?
                    .commit_live_ledger(initial, current_fence())
                    .await?,
                LiveLedgerCommitOutcome::Committed { .. }
            ));
            let outcomes = if *outcome == LivePhysicalEffectOutcome::Unknown {
                vec![*outcome, LivePhysicalEffectOutcome::Succeeded]
            } else {
                vec![*outcome]
            };
            for (index, outcome) in outcomes.into_iter().enumerate() {
                let before = fixture
                    .ops()?
                    .load_live_head(fixture.session.id())
                    .await?
                    .ok_or("head")?;
                let completion = record(
                    fixture.session.id(),
                    request.clone(),
                    claim.clone(),
                    index as u64 + 1,
                    outcome,
                )?;
                Mutator::apply(&mut owner, settlement(&completion)?)?;
                let mut commit =
                    PreparedLiveLedgerCommit::from_request_transition_with_completions(
                        fixture.session.id(),
                        Some(&before),
                        &owner.prepare_authority(),
                        vec![completion.clone()],
                    )?;
                commit.quota = quota;
                let used_and_reserved = commit
                    .successor
                    .payload
                    .used
                    .checked_add(commit.successor.payload.reserved)?;
                if outcome == LivePhysicalEffectOutcome::NotStarted {
                    assert!(used_and_reserved.fits_within(quota));
                    assert_eq!(
                        commit.successor.payload.reserved, request_reservation,
                        "proven noninvocation releases effect credits without spending the request obligation"
                    );
                } else {
                    assert_eq!(
                        used_and_reserved, quota,
                        "snapshot growth must consume reserved slack, not new quota"
                    );
                    assert!(commit.successor.payload.reserved.records > 0);
                    assert!(
                        commit.successor.payload.reserved.records > request_reservation.records,
                        "observed tools retain callback capacity until ordinary classification",
                    );
                }

                let replay = copy_prepared(&commit);
                assert!(matches!(
                    fixture
                        .ops()?
                        .commit_live_ledger(commit, current_fence())
                        .await?,
                    LiveLedgerCommitOutcome::Committed { .. }
                ));
                assert!(matches!(
                    fixture
                        .ops()?
                        .commit_live_ledger(replay, current_fence())
                        .await?,
                    LiveLedgerCommitOutcome::AlreadyCommitted { .. }
                ));
                let head = fixture
                    .ops()?
                    .load_live_head(fixture.session.id())
                    .await?
                    .ok_or("settled head")?;
                assert_eq!(head.reference.event_count, index as u64 + 1);
                let decoded =
                    crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
                assert_eq!(
                    decoded.claim_credit_spent_records.get(&claim.to_string()),
                    Some(&(index as u64 + 1))
                );
                assert_eq!(
                    decoded.claim_terminal_digests.get(&claim.to_string()),
                    Some(&completion_digest(&completion)?)
                );
            }
        }
    }
    Ok(())
}

#[test]
fn generated_settlement_cannot_commit_without_its_actual_completion_record() -> TestResult {
    let session_id = SessionId::new();
    let (owner, request, claim) = claimed()?;
    let initial = PreparedLiveLedgerCommit::from_request_transition(
        &session_id,
        None,
        &owner.prepare_authority(),
    )?;
    let record = record(
        &session_id,
        request,
        claim,
        1,
        LivePhysicalEffectOutcome::Unknown,
    )?;
    let mut candidate = owner.prepare_authority();
    Mutator::apply(&mut candidate, settlement(&record)?)?;
    assert!(
        PreparedLiveLedgerCommit::from_request_transition(
            &session_id,
            Some(&initial.successor),
            &candidate,
        )
        .is_err()
    );
    PreparedLiveLedgerCommit::from_request_transition_with_completions(
        &session_id,
        Some(&initial.successor),
        &candidate,
        vec![record],
    )?;
    Ok(())
}

#[test]
fn generated_unknown_retains_late_feedback_capacity_and_local_noninvocation_is_not_recoverable()
-> TestResult {
    let (owner, request, claim) = claimed()?;
    let completion = record(
        &SessionId::new(),
        request,
        claim.clone(),
        1,
        LivePhysicalEffectOutcome::Unknown,
    )?;
    for corruption in 0..4 {
        let mut input = settlement(&completion)?;
        let Input::SettleEffect {
            completion_records,
            completion_bytes,
            completion_sequence,
            outcome,
            local_noninvocation_proven,
            ..
        } = &mut input
        else {
            return Err("expected settlement".into());
        };
        match corruption {
            0 => {
                let budget = EffectCompletionBudget::measured()?.envelope.total();
                *completion_records = budget.records;
                *completion_bytes = budget.encoded_bytes;
            }
            1 => *completion_sequence = u64::MAX,
            2 => {
                *outcome = LiveEffectPhase::NotStarted;
                *local_noninvocation_proven = false;
            }
            3 => *completion_bytes = u64::MAX,
            _ => return Err("unexpected case".into()),
        }
        assert!(
            Mutator::apply(&mut owner.prepare_authority(), input).is_err(),
            "case {corruption}"
        );
    }
    let mut owner = owner;
    Mutator::apply(&mut owner, settlement(&completion)?)?;
    let mut recovered = Authority::recover_from_state(owner.state().clone())?;
    let mut late = settlement(&completion)?;
    let Input::SettleEffect {
        outcome,
        completion_sequence,
        local_noninvocation_proven,
        ..
    } = &mut late
    else {
        return Err("expected settlement".into());
    };
    *outcome = LiveEffectPhase::NotStarted;
    *completion_sequence = 2;
    *local_noninvocation_proven = true;
    assert!(Mutator::apply(&mut recovered, late).is_err());
    assert_eq!(
        recovered.state().claim_phases.get(&claim.to_string()),
        Some(&LiveEffectPhase::Unknown)
    );
    Ok(())
}

#[test]
fn completion_append_requires_exact_generated_bytes_charge_identity_and_terminal_witness()
-> TestResult {
    let session = SessionId::new();
    let (mut owner, request, claim) = claimed()?;
    let initial = PreparedLiveLedgerCommit::from_request_transition(
        &session,
        None,
        &owner.prepare_authority(),
    )?;
    let completion = record(
        &session,
        request,
        claim,
        1,
        LivePhysicalEffectOutcome::Unknown,
    )?;
    Mutator::apply(&mut owner, settlement(&completion)?)?;
    for field in 0..5 {
        let mut changed = completion.clone();
        match field {
            0 => changed.session_id = SessionId::new(),
            1 => changed.channel_id = LiveChannelId::new("different-channel"),
            2 => changed.sequence = LiveObservationSeq::new(2)?,
            3 | 4 => {
                let LiveCompletionEvent::EffectTerminal {
                    outcome,
                    diagnostic,
                    ..
                } = &mut changed.event
                else {
                    return Err("expected completion".into());
                };
                if field == 3 {
                    *outcome = LivePhysicalEffectOutcome::Succeeded;
                } else {
                    *diagnostic = LiveCompletionText::new("different bytes")?;
                }
            }
            _ => return Err("unexpected field".into()),
        }
        assert!(
            PreparedLiveLedgerCommit::from_request_transition_with_completions(
                &session,
                Some(&initial.successor),
                &owner.prepare_authority(),
                vec![changed],
            )
            .is_err(),
            "field {field}"
        );
    }
    Ok(())
}

#[test]
fn stored_effect_credit_images_reject_overspend_zero_divisors_and_unmeasured_budgets() -> TestResult
{
    let (owner, _, claim) = claimed()?;
    for corruption in 0..5 {
        let mut state = owner.state().clone();
        let claim = claim.to_string();
        match corruption {
            0 => {
                state.claim_credit_maximum_record_charge.insert(claim, 0);
            }
            1 => {
                state.claim_credit_records.insert(claim, u64::MAX);
            }
            2 => {
                state.claim_credit_spent_bytes.insert(claim, u64::MAX);
            }
            3 => {
                state
                    .claim_terminal_digests
                    .insert(claim, "uncommitted".into());
            }
            4 => {
                state.claim_credit_snapshot_ceiling.clear();
            }
            _ => return Err("unexpected corruption".into()),
        }
        assert!(
            Authority::recover_from_state(state).is_err(),
            "case {corruption}"
        );
    }
    let mut unmeasured = owner.state().clone();
    unmeasured
        .claim_credit_snapshot_ceiling
        .insert(claim.to_string(), 1);
    let valid_machine = Authority::recover_from_state(unmeasured)?;
    assert!(reserved_completion_charge(valid_machine.state()).is_err());
    Ok(())
}
