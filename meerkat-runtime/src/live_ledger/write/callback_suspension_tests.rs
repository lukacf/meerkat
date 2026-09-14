use super::claim_tests::{read_only_observation, tool_target_for_call};
use super::request_completion_tests::seed_finalized_ordinary;
use super::restore_tests::scope_owner;
use super::*;
use crate::completion::CompletionOutcome;
use crate::input_state::InputStatePersistenceRecord;
use crate::live_ledger::authority::store::LiveRequestAuthorityError;
use crate::live_ledger::authority::store::request_completion::LiveRequestCompletionProgress;
use crate::live_ledger::completion::{LiveCompletionText, LivePhysicalEffectOutcome};
use meerkat_core::execution_scope::ScopedRunAuthority;
use meerkat_core::session::CallbackBatchIdentity;

async fn suspended_authority() -> TestResult<(dsl::LiveRequestMachineAuthority, ScopedRunAuthority)>
{
    let (owned, scope) = pending_batch(Backend::Memory).await?;
    scope_owner(&owned)
        .reconcile_request_completion(&scope.record().request_id)
        .await?;
    let head = owned
        .fixture
        .ops()?
        .load_live_head(owned.fixture.session.id())
        .await?
        .ok_or("head")?;
    Ok((
        dsl::LiveRequestMachineAuthority::recover_from_state(
            crate::generated::live_request_state::decode(&head.payload.request_snapshot)?,
        )?,
        scope,
    ))
}

#[tokio::test]
async fn cancelled_callback_hold_observation_requires_exact_retained_authorities() -> TestResult {
    use meerkat_core::live_execution::request::LiveRequestCancellationReason;
    let (owner, scope) = suspended_authority().await?;
    let request = scope.record().request_id.to_string();
    let run = scope.record().run_id.to_string();
    let command = dsl::LiveRequestInput::ObserveCancelledCallback {
        request_id: request.clone(),
        run_id: run.clone(),
        callback_record: owner.state().run_callback_records[&run].clone(),
        ordinary_completion_digest: owner.state().run_callback_receipts[&run].clone(),
        callback_claims: owner.state().run_callback_claims[&run].clone(),
        completion_digest: owner.state().run_callback_digests[&run].clone(),
        reason: LiveRequestCancellationReason::OperatorRequested,
    };
    assert!(
        dsl::LiveRequestMachineMutator::apply(&mut owner.prepare_authority(), command.clone())
            .is_err(),
        "a callback without source cancellation cannot be a cancellation hold"
    );
    let cancel = dsl::LiveRequestInput::CancelSource {
        source: owner.state().request_sources[&request].clone(),
        reason: LiveRequestCancellationReason::OperatorRequested,
    };
    let mut cancelled = owner.prepare_authority();
    dsl::LiveRequestMachineMutator::apply(&mut cancelled, cancel.clone())?;
    let before = cancelled.state().clone();
    let observed = dsl::LiveRequestMachineMutator::apply(&mut cancelled, command.clone())?;
    assert!(matches!(observed.effects(),
        [dsl::LiveRequestEffect::CancelledCallbackHeld { request_id, run_id, reason }]
            if request_id == &request && run_id == &run
                && *reason == LiveRequestCancellationReason::OperatorRequested));
    assert_eq!(
        cancelled.state(),
        &before,
        "a held observation cannot mutate callback truth or credits"
    );
    for corruption in 0..7 {
        let mut damaged = command.clone();
        let dsl::LiveRequestInput::ObserveCancelledCallback {
            request_id,
            run_id,
            callback_record,
            ordinary_completion_digest,
            callback_claims,
            completion_digest,
            reason,
        } = &mut damaged
        else {
            unreachable!()
        };
        match corruption {
            0 => *request_id = "foreign-request".into(),
            1 => *run_id = "foreign-run".into(),
            2 => *callback_record = "foreign-callback".into(),
            3 => *ordinary_completion_digest = "0".repeat(64),
            4 => callback_claims.clear(),
            5 => *completion_digest = "0".repeat(64),
            6 => *reason = LiveRequestCancellationReason::GrantRevoked,
            _ => unreachable!(),
        }
        assert!(
            dsl::LiveRequestMachineMutator::apply(&mut cancelled, damaged).is_err(),
            "corruption {corruption}"
        );
        assert_eq!(cancelled.state(), &before);
    }
    let mut admitted = owner.prepare_authority();
    dsl::LiveRequestMachineMutator::apply(&mut admitted, continuation_admission(&owner, &scope)?)?;
    dsl::LiveRequestMachineMutator::apply(&mut admitted, cancel)?;
    assert!(
        dsl::LiveRequestMachineMutator::apply(&mut admitted, command).is_err(),
        "an admitted continuation must follow its own ordinary input, not the old callback hold"
    );
    Ok(())
}

#[tokio::test]
async fn native_cancelled_callback_hold_reports_original_receipt_without_spend() -> TestResult {
    use meerkat_core::live_execution::request::{
        LiveRequestCancelIntent, LiveRequestCancellationReason, LiveSourceKey,
    };
    for backend in backends() {
        let (owned, scope) = pending_batch(backend).await?;
        let owner = scope_owner(&owned);
        owner
            .reconcile_request_completion(&scope.record().request_id)
            .await?;
        let ops = owned.fixture.ops()?;
        let head = ops
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        let request = scope.record().request_id.to_string();
        let run = scope.record().run_id.to_string();
        let source: LiveSourceKey = serde_json::from_str(&state.request_sources[&request])?;
        for reason in [
            LiveRequestCancellationReason::OperatorRequested,
            LiveRequestCancellationReason::GrantRevoked,
        ] {
            owner
                .cancel_source(LiveRequestCancelIntent {
                    source: source.clone(),
                    reason,
                })
                .await?;
            let before = ops.load_live_head(owned.fixture.session.id()).await?;
            let LiveRequestCompletionProgress::CancelledCallbackHeld(hold) = owner
                .reconcile_request_completion(&scope.record().request_id)
                .await?
            else {
                return Err("expected explicit cancelled callback hold".into());
            };
            assert_eq!(
                hold.reason,
                LiveRequestCancellationReason::OperatorRequested
            );
            assert_eq!(
                hold.ordinary_receipt_digest,
                state.run_callback_receipts[&run]
            );
            assert_eq!(hold.input_id, scope.record().input_id);
            assert_eq!(hold.callback.run_id(), &scope.record().run_id);
            assert_eq!(
                hold.callback_sequence.get(),
                state.run_callback_sequences[&run]
            );
            assert!(owner.pending_source_cancellations().await?.is_empty());
            assert_eq!(
                ops.load_live_head(owned.fixture.session.id()).await?,
                before,
                "observation must retain original phase, unknown effects, and all outstanding credits"
            );
        }
    }
    Ok(())
}

fn continuation_admission(
    owner: &dsl::LiveRequestMachineAuthority,
    scope: &ScopedRunAuthority,
) -> TestResult<dsl::LiveRequestInput> {
    Ok(dsl::LiveRequestInput::AdmitCallbackContinuation {
        request_id: scope.record().request_id.to_string(),
        run_id: scope.record().run_id.to_string(),
        callback_record: owner
            .state()
            .run_callback_records
            .get(&scope.record().run_id.to_string())
            .ok_or("callback record")?
            .clone(),
        result_digest: "a".repeat(64),
        input_id: "continuation-input".into(),
        admission_commit: "continuation-admission".into(),
        executor: owner.state().executor_binding.clone(),
        profile_revision: owner.state().grant_profile_revision.clone(),
        stage_credit_bytes: 1000,
        now: owner.state().grant_expiry - 1,
    })
}

fn continuation_stage(
    owner: &dsl::LiveRequestMachineAuthority,
    scope: &ScopedRunAuthority,
) -> dsl::LiveRequestInput {
    dsl::LiveRequestInput::StageCallbackContinuation {
        request_id: scope.record().request_id.to_string(),
        previous_run_id: scope.record().run_id.to_string(),
        input_id: "continuation-input".into(),
        admission_commit: "continuation-admission".into(),
        result_digest: "a".repeat(64),
        run_id: "continuation-run".into(),
        scope_id: "continuation-scope".into(),
        scope_record: "continuation-scope-record".into(),
        executor: owner.state().executor_binding.clone(),
        profile_revision: owner.state().grant_profile_revision.clone(),
        now: owner.state().grant_expiry - 1,
    }
}

// This checks materialized content against a generated admission image, not
// callback readiness or the native joint-admission producer.
#[tokio::test]
async fn callback_input_materializes_no_prompt_and_checks_exact_chain_receipt() -> TestResult {
    use crate::live_request::{LiveExecutionRequestRecord, LiveRequestMaterializationError};
    use meerkat_core::execution_scope::{
        ExecutionAdmissionCommitRef, ScopedCallbackContinuationRecord,
    };
    use meerkat_core::lifecycle::InputId;

    for backend in backends() {
        let (owned, scope) = pending_batch(backend).await?;
        scope_owner(&owned)
            .reconcile_request_completion(&scope.record().request_id)
            .await?;
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let owner = dsl::LiveRequestMachineAuthority::recover_from_state(
            crate::generated::live_request_state::decode(&head.payload.request_snapshot)?,
        )?;
        let run = scope.record().run_id.to_string();
        let target: CallbackBatchIdentity = serde_json::from_str(
            owner
                .state()
                .run_callback_records
                .get(&run)
                .ok_or("callback")?,
        )?;
        let source: meerkat_core::live_execution::request::LiveSourceKey = serde_json::from_str(
            owner
                .state()
                .request_sources
                .get(&scope.record().request_id.to_string())
                .ok_or("source")?,
        )?;
        let row = owned
            .fixture
            .ops()?
            .lookup_live_source(&source)
            .await?
            .ok_or("source row")?;
        let crate::live_source::LiveSourceEntryRecord::Reservation { record } = row.record()?
        else {
            return Err("source reservation".into());
        };
        let evidence = record.frozen_request().ok_or("frozen request")?;
        let input_id = InputId::new();
        let admission_commit = ExecutionAdmissionCommitRef {
            revision: std::num::NonZeroU64::new(head.reference.revision + 1).ok_or("revision")?,
            digest: [17; 32],
        };
        let reference = LiveExecutionRequestRecord::CallbackContinuation {
            provenance: meerkat_core::live_execution::evidence::DelegatedRequestProvenance::new(
                record.request_id().clone(),
                source,
                evidence.kind(),
                evidence.request().digest(),
            )?,
            source_row: record.frozen_digest()?,
            continuation: ScopedCallbackContinuationRecord {
                target: target.clone(),
                results_digest: [0xaa; 32],
            },
            admission_commit: admission_commit.clone(),
        };
        assert!(matches!(
            reference
                .materialize(owned.fixture.store.as_ref(), &input_id)
                .await,
            Err(LiveRequestMaterializationError::ContinuationMismatch)
        ));
        let mut candidate = owner.prepare_authority();
        dsl::LiveRequestMachineMutator::apply(
            &mut candidate,
            dsl::LiveRequestInput::AdmitCallbackContinuation {
                request_id: scope.record().request_id.to_string(),
                run_id: run,
                callback_record: serde_json::to_string(&target)?,
                result_digest: "a".repeat(64),
                input_id: input_id.to_string(),
                admission_commit: serde_json::to_string(&admission_commit)?,
                executor: owner.state().executor_binding.clone(),
                profile_revision: owner.state().grant_profile_revision.clone(),
                stage_credit_bytes: 1000,
                now: owner.state().grant_expiry - 1,
            },
        )?;
        let prepared = PreparedLiveLedgerCommit::from_request_transition(
            owned.fixture.session.id(),
            Some(&head),
            &candidate,
        )?;
        for source_changed in [false, true] {
            let mut changed = candidate.state().clone();
            let map = if source_changed {
                &mut changed.request_sources
            } else {
                &mut changed.request_payloads
            };
            map.insert(
                scope.record().request_id.to_string(),
                "different source binding".into(),
            );
            assert!(
                reference
                    .validate_continuation_binding(&changed, &input_id)
                    .is_err()
            );
        }
        owned
            .fixture
            .ops()?
            .commit_live_ledger(prepared, current_fence())
            .await?;
        assert!(
            reference
                .materialize(owned.fixture.store.as_ref(), &input_id)
                .await?
                .is_none()
        );
        assert_eq!(
            owned
                .fixture
                .ops()?
                .lookup_live_source(record.source())
                .await?
                .ok_or("source")?
                .digest(),
            row.digest(),
            "continuation must not replace the source's original admission"
        );
        assert!(matches!(
            reference
                .materialize(owned.fixture.store.as_ref(), &InputId::new())
                .await,
            Err(LiveRequestMaterializationError::ContinuationMismatch)
        ));
        let encoded = serde_json::to_value(&reference)?;
        for path in [
            "session_id",
            "run_id",
            "execution_scope",
            "execution_boundary",
        ] {
            let mut changed = encoded.clone();
            changed["continuation"]["target"][path] = serde_json::json!(uuid::Uuid::new_v4());
            let changed: LiveExecutionRequestRecord = serde_json::from_value(changed)?;
            assert!(
                matches!(
                    changed
                        .materialize(owned.fixture.store.as_ref(), &input_id)
                        .await,
                    Err(LiveRequestMaterializationError::ContinuationMismatch)
                ),
                "{path}"
            );
        }
        for field in ["batch_digest", "results_digest", "admission_commit"] {
            let mut changed = encoded.clone();
            match field {
                "batch_digest" => {
                    changed["continuation"]["target"][field] = serde_json::json!(vec![19; 32]);
                }
                "results_digest" => {
                    changed["continuation"][field] = serde_json::json!(vec![19; 32]);
                }
                _ => changed[field]["digest"] = serde_json::json!(vec![19; 32]),
            }
            let changed: LiveExecutionRequestRecord = serde_json::from_value(changed)?;
            assert!(
                matches!(
                    changed
                        .materialize(owned.fixture.store.as_ref(), &input_id)
                        .await,
                    Err(LiveRequestMaterializationError::ContinuationMismatch)
                ),
                "{field}"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn generated_callback_continuation_is_one_shot_and_preserves_original_admission() -> TestResult
{
    use crate::live_ledger::authority::store::request_credits::reserved_completion_charge;

    let (owner, scope) = suspended_authority().await?;
    let before_credit = reserved_completion_charge(owner.state())?;
    let request = scope.record().request_id.to_string();
    let run = scope.record().run_id.to_string();
    let original_inputs = owner.state().request_inputs.clone();
    let original_admissions = owner.state().request_admission_commits.clone();
    let admission = continuation_admission(&owner, &scope)?;
    let stage = continuation_stage(&owner, &scope);
    let mut candidate = owner.prepare_authority();
    assert!(dsl::LiveRequestMachineMutator::apply(&mut candidate, stage.clone()).is_err());
    dsl::LiveRequestMachineMutator::apply(&mut candidate, dsl::LiveRequestInput::CloseIngress)?;
    let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, admission.clone())?;
    assert!(matches!(
        transition.effects(),
        [dsl::LiveRequestEffect::CallbackContinuationAdmitted { .. }]
    ));
    assert_eq!(
        reserved_completion_charge(candidate.state())?,
        before_credit.checked_add(crate::live_resources::LiveResourceCharge {
            records: 0,
            encoded_bytes: 1000,
        })?
    );
    assert!(dsl::LiveRequestMachineMutator::apply(&mut candidate, admission).is_err());
    let serialized = crate::generated::live_request_state::encode(candidate.state())?;
    let recovered = dsl::LiveRequestMachineAuthority::recover_from_state(
        crate::generated::live_request_state::decode(&serialized)?,
    )?;
    let mut candidate = recovered.prepare_authority();
    assert_eq!(
        reserved_completion_charge(candidate.state())?.encoded_bytes,
        before_credit.encoded_bytes + 1000
    );
    let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, stage.clone())?;
    assert!(matches!(
        transition.effects(),
        [dsl::LiveRequestEffect::RunScopeBound { run_id, .. }]
            if run_id == "continuation-run"
    ));
    assert!(dsl::LiveRequestMachineMutator::apply(&mut candidate, stage).is_err());
    assert_eq!(candidate.state().request_inputs, original_inputs);
    assert_eq!(
        candidate.state().request_admission_commits,
        original_admissions
    );
    assert_eq!(
        candidate
            .state()
            .request_runs
            .get(&request)
            .map(String::as_str),
        Some("continuation-run")
    );
    assert_eq!(
        candidate.state().run_inputs.get(&run),
        Some(&scope.record().input_id.to_string())
    );
    assert_eq!(
        candidate.state().run_predecessors.get("continuation-run"),
        Some(&run)
    );
    assert_eq!(
        candidate.state().run_ordinals.get("continuation-run"),
        Some(&1)
    );
    assert_eq!(
        candidate.state().run_continuation_stage_credits.get(&run),
        Some(&0)
    );
    let phase_shrink = serde_json::to_vec(&dsl::LiveRequestPhase::Suspended)?.len()
        - serde_json::to_vec(&dsl::LiveRequestPhase::Running)?.len();
    assert_eq!(
        reserved_completion_charge(candidate.state())?.encoded_bytes,
        before_credit.encoded_bytes + phase_shrink as u64
    );

    for corruption in 0..7 {
        let mut damaged = candidate.state().clone();
        match corruption {
            0 => {
                damaged
                    .run_predecessors
                    .insert("continuation-run".into(), "continuation-run".into());
            }
            1 => {
                damaged.run_ordinals.insert("continuation-run".into(), 0);
            }
            2 => {
                damaged.run_successors.insert(run.clone(), String::new());
            }
            3 => {
                damaged
                    .run_continuation_result_digests
                    .insert(run.clone(), String::new());
            }
            4 => {
                damaged
                    .run_continuation_stage_credits
                    .insert(run.clone(), 1000);
            }
            5 => {
                damaged.run_inputs.insert(
                    "continuation-run".into(),
                    scope.record().input_id.to_string(),
                );
            }
            _ => {
                damaged.request_runs.insert(request.clone(), run.clone());
            }
        }
        assert!(dsl::LiveRequestMachineAuthority::recover_from_state(damaged).is_err());
    }
    Ok(())
}

#[tokio::test]
async fn generated_callback_continuation_keeps_cancel_revoke_expiry_and_target_fences() -> TestResult
{
    let (owner, scope) = suspended_authority().await?;
    for mutation in 0..8 {
        let mut command = continuation_admission(&owner, &scope)?;
        let dsl::LiveRequestInput::AdmitCallbackContinuation {
            run_id,
            callback_record,
            result_digest,
            input_id,
            admission_commit,
            executor,
            stage_credit_bytes,
            now,
            ..
        } = &mut command
        else {
            return Err("admission command".into());
        };
        match mutation {
            0 => *run_id = "foreign".into(),
            1 => *callback_record = "foreign".into(),
            2 => *result_digest = "short".into(),
            3 => *input_id = scope.record().input_id.to_string(),
            4 => {
                *admission_commit = owner
                    .state()
                    .request_admission_commits
                    .get(&scope.record().request_id.to_string())
                    .ok_or("original admission")?
                    .clone();
            }
            5 => *executor = "foreign".into(),
            6 => *stage_credit_bytes = 0,
            _ => *now = owner.state().grant_expiry,
        }
        let mut candidate = owner.prepare_authority();
        assert!(dsl::LiveRequestMachineMutator::apply(&mut candidate, command).is_err());
    }
    for fence in [
        dsl::LiveRequestInput::Cancel {
            request_id: scope.record().request_id.to_string(),
        },
        dsl::LiveRequestInput::Revoke {
            grant_id: owner.state().grant_id.clone(),
            generation: owner.state().grant_generation,
        },
    ] {
        let admission = continuation_admission(&owner, &scope)?;
        let mut before_admission = owner.prepare_authority();
        dsl::LiveRequestMachineMutator::apply(&mut before_admission, fence.clone())?;
        assert!(
            dsl::LiveRequestMachineMutator::apply(&mut before_admission, admission.clone())
                .is_err()
        );
        let mut after_admission = owner.prepare_authority();
        dsl::LiveRequestMachineMutator::apply(&mut after_admission, admission)?;
        dsl::LiveRequestMachineMutator::apply(&mut after_admission, fence)?;
        assert!(
            dsl::LiveRequestMachineMutator::apply(
                &mut after_admission,
                continuation_stage(&owner, &scope),
            )
            .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn generated_callback_continuation_replay_is_history_not_scope_permission() -> TestResult {
    let (owner, scope) = suspended_authority().await?;
    let request_id = scope.record().request_id.to_string();
    let run_id = scope.record().run_id.to_string();
    let state = owner.state();
    let observe = dsl::LiveRequestInput::ObserveCallbackContinuation {
        request_id: request_id.clone(),
        run_id: run_id.clone(),
        callback_record: state
            .run_callback_records
            .get(&run_id)
            .ok_or("callback")?
            .clone(),
        result_digest: "a".repeat(64),
        input_id: "continuation-input".into(),
        admission_commit: "continuation-admission".into(),
    };
    let old_restore = dsl::LiveRequestInput::RestoreScope {
        request_id: request_id.clone(),
        input_id: scope.record().input_id.to_string(),
        admission_commit: state
            .run_admission_commits
            .get(&run_id)
            .ok_or("admission")?
            .clone(),
        run_id: run_id.clone(),
        scope_id: state.run_scopes.get(&run_id).ok_or("scope")?.clone(),
        scope_record: state
            .run_scope_records
            .get(&run_id)
            .ok_or("scope record")?
            .clone(),
        parent_scope: state
            .request_parents
            .get(&request_id)
            .ok_or("parent")?
            .clone(),
        executor: state.executor_binding.clone(),
        profile_revision: state.grant_profile_revision.clone(),
        now: state.grant_expiry - 1,
    };
    let mut candidate = owner.prepare_authority();
    assert!(dsl::LiveRequestMachineMutator::apply(&mut candidate, observe.clone()).is_err());
    dsl::LiveRequestMachineMutator::apply(&mut candidate, continuation_admission(&owner, &scope)?)?;
    dsl::LiveRequestMachineMutator::apply(&mut candidate, continuation_stage(&owner, &scope))?;
    assert!(dsl::LiveRequestMachineMutator::apply(&mut candidate, old_restore).is_err());

    for fence in [
        None,
        Some(dsl::LiveRequestInput::Cancel {
            request_id: request_id.clone(),
        }),
        Some(dsl::LiveRequestInput::Revoke {
            grant_id: state.grant_id.clone(),
            generation: state.grant_generation,
        }),
    ] {
        if let Some(fence) = fence {
            dsl::LiveRequestMachineMutator::apply(&mut candidate, fence)?;
        }
        let before = crate::generated::live_request_state::encode(candidate.state())?;
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, observe.clone())?;
        assert!(matches!(
            transition.effects(),
            [dsl::LiveRequestEffect::CallbackContinuationObserved { .. }]
        ));
        assert_eq!(
            crate::generated::live_request_state::encode(candidate.state())?,
            before
        );
        assert!(
            dsl::LiveRequestMachineMutator::apply(
                &mut candidate,
                continuation_stage(&owner, &scope),
            )
            .is_err()
        );
    }
    for mutation in 0..6 {
        let mut wrong = observe.clone();
        let dsl::LiveRequestInput::ObserveCallbackContinuation {
            request_id,
            run_id,
            callback_record,
            result_digest,
            input_id,
            admission_commit,
        } = &mut wrong
        else {
            return Err("observe command".into());
        };
        match mutation {
            0 => *request_id = "foreign".into(),
            1 => *run_id = "foreign".into(),
            2 => *callback_record = "foreign".into(),
            3 => *result_digest = "b".repeat(64),
            4 => *input_id = "foreign".into(),
            _ => *admission_commit = "foreign".into(),
        }
        assert!(dsl::LiveRequestMachineMutator::apply(&mut candidate, wrong).is_err());
    }
    Ok(())
}

#[tokio::test]
async fn generated_callback_application_claim_is_one_shot_without_new_tool_or_token_charge()
-> TestResult {
    let (owner, scope) = suspended_authority().await?;
    let mut staged = owner.prepare_authority();
    dsl::LiveRequestMachineMutator::apply(&mut staged, continuation_admission(&owner, &scope)?)?;
    dsl::LiveRequestMachineMutator::apply(&mut staged, continuation_stage(&owner, &scope))?;
    let staged = dsl::LiveRequestMachineAuthority::recover_from_state(staged.state().clone())?;
    let command = dsl::LiveRequestInput::ClaimCallbackApplication {
        request_id: scope.record().request_id.to_string(),
        run_id: "continuation-run".into(),
        input_id: "continuation-input".into(),
        admission_commit: "continuation-admission".into(),
        scope_id: "continuation-scope".into(),
        scope_record: "continuation-scope-record".into(),
        callback_record: owner
            .state()
            .run_callback_records
            .get(&scope.record().run_id.to_string())
            .ok_or("callback")?
            .clone(),
        result_digest: "a".repeat(64),
        executor: owner.state().executor_binding.clone(),
        profile_revision: owner.state().grant_profile_revision.clone(),
        now: owner.state().grant_expiry - 1,
    };
    let mut candidate = staged.prepare_authority();
    let before = crate::generated::live_request_state::encode(candidate.state())?;
    let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone())?;
    assert!(matches!(
        transition.effects(),
        [dsl::LiveRequestEffect::CallbackApplicationClaimed { .. }]
    ));
    assert_eq!(
        candidate.state().remaining_effects,
        staged.state().remaining_effects
    );
    assert_eq!(
        candidate.state().request_known_tokens,
        staged.state().request_known_tokens
    );
    assert_eq!(candidate.state().claim_ids, staged.state().claim_ids);
    let after = crate::generated::live_request_state::encode(candidate.state())?;
    assert_eq!(
        after.len() + 1,
        before.len(),
        "only false -> true changes the stored image"
    );
    let mut recovered = dsl::LiveRequestMachineAuthority::recover_from_state(
        crate::generated::live_request_state::decode(&after)?,
    )?;
    assert!(dsl::LiveRequestMachineMutator::apply(&mut recovered, command.clone()).is_err());
    for mutation in 0..8 {
        let mut wrong = command.clone();
        let dsl::LiveRequestInput::ClaimCallbackApplication {
            run_id,
            input_id,
            callback_record,
            result_digest,
            scope_record,
            executor,
            profile_revision,
            now,
            ..
        } = &mut wrong
        else {
            return Err("claim command".into());
        };
        match mutation {
            0 => *run_id = scope.record().run_id.to_string(),
            1 => *input_id = scope.record().input_id.to_string(),
            2 => *callback_record = "foreign".into(),
            3 => *result_digest = "b".repeat(64),
            4 => *scope_record = "foreign".into(),
            5 => *executor = "foreign".into(),
            6 => *profile_revision = "foreign".into(),
            _ => *now = owner.state().grant_expiry,
        }
        assert!(
            dsl::LiveRequestMachineMutator::apply(&mut staged.prepare_authority(), wrong).is_err()
        );
    }
    for fence in [
        dsl::LiveRequestInput::Cancel {
            request_id: scope.record().request_id.to_string(),
        },
        dsl::LiveRequestInput::Revoke {
            grant_id: owner.state().grant_id.clone(),
            generation: owner.state().grant_generation,
        },
        dsl::LiveRequestInput::FenceExecutor {
            executor: "replacement".into(),
        },
    ] {
        let mut candidate = staged.prepare_authority();
        dsl::LiveRequestMachineMutator::apply(&mut candidate, fence)?;
        assert!(dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone()).is_err());
    }
    Ok(())
}

#[tokio::test]
async fn generated_callback_suspension_rejects_lost_membership_receipts_and_spend() -> TestResult {
    let (owned, scope) = pending_batch(Backend::Memory).await?;
    scope_owner(&owned)
        .reconcile_request_completion(&scope.record().request_id)
        .await?;
    let head = owned
        .fixture
        .ops()?
        .load_live_head(owned.fixture.session.id())
        .await?
        .ok_or("head")?;
    let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
    let run = scope.record().run_id.to_string();
    let claim = state.claim_ids.first().ok_or("claim")?;
    for corruption in 0..13 {
        let mut damaged = state.clone();
        match corruption {
            0 => {
                damaged.run_callback_records.remove(&run);
            }
            1 => {
                damaged.run_callback_receipts.remove(&run);
            }
            2 => {
                damaged.run_callback_claims.remove(&run);
            }
            3 => {
                damaged.run_callback_sequences.remove(&run);
            }
            4 => {
                damaged.run_callback_digests.remove(&run);
            }
            5 => {
                damaged
                    .run_callback_records
                    .insert(run.clone(), String::new());
            }
            6 => {
                damaged
                    .run_callback_receipts
                    .insert(run.clone(), "wrong".into());
            }
            7 => {
                damaged
                    .run_callback_claims
                    .insert(run.clone(), Default::default());
            }
            8 => {
                damaged.run_callback_sequences.insert(run.clone(), 0);
            }
            9 => {
                damaged
                    .run_callback_digests
                    .insert(run.clone(), "wrong".into());
            }
            10 => {
                damaged
                    .run_callback_claims
                    .get_mut(&run)
                    .ok_or("members")?
                    .insert("foreign".into());
            }
            11 => {
                damaged
                    .claim_phases
                    .insert(claim.clone(), dsl::LiveEffectPhase::NotStarted);
            }
            12 => {
                damaged.claim_credit_spent_records.insert(claim.clone(), 1);
            }
            _ => unreachable!(),
        }
        assert!(
            dsl::LiveRequestMachineAuthority::recover_from_state(damaged).is_err(),
            "corruption {corruption}"
        );
    }
    Ok(())
}

async fn pending_batch(backend: Backend) -> TestResult<(OwnedFixture, ScopedRunAuthority)> {
    let owned = OwnedFixture::new(backend).await?;
    let scope = owned.staged_scope().await?;
    let identity: CallbackBatchIdentity = serde_json::from_value(serde_json::json!({
        "session_id": owned.fixture.session.id(),
        "run_id": scope.record().run_id,
        "execution_scope": scope.scope_id(),
        "execution_boundary": meerkat_core::ops::OperationId::new(),
        "batch_digest": vec![255; 32],
    }))?;
    let mut pending_tool_calls = Vec::new();
    for call in ["second", "first"] {
        let permit = owned
            .machine
            .claim_live_effect(
                scope.clone(),
                identity.scoped_tool_effect_id(call)?.ok_or("effect")?,
                tool_target_for_call(
                    call,
                    "allowed_tool",
                    meerkat_core::ToolMutationClass::ReadOnly,
                ),
                read_only_observation()?,
            )
            .await?;
        owned
            .machine
            .settle_live_effect(
                permit.into_claim(),
                LivePhysicalEffectOutcome::Unknown,
                LiveCompletionText::new("callback observed")?,
            )
            .await?;
        pending_tool_calls.push(meerkat_core::error::PendingCallbackToolCall {
            tool_use_id: call.into(),
            tool_name: "callback_view".into(),
            args: serde_json::json!({"escaped": "\0\"\\\n"}),
        });
    }
    seed_finalized_ordinary(
        &owned,
        &scope,
        CompletionOutcome::CallbackBatchPending {
            pending_tool_calls,
            callback_identity: Some(identity),
        },
    )
    .await?;
    Ok((owned, scope))
}

#[tokio::test]
async fn native_callback_batch_cannot_omit_duplicate_or_replace_completion_records() -> TestResult {
    use crate::live_ledger::completion::{LiveCompletionEvent, LiveCompletionRecord};
    use meerkat_core::live_execution::evidence::LiveContentDigest;
    use meerkat_core::live_observation::LiveObservationSeq;
    use meerkat_core::ops::OperationId;
    let (owned, scope) = pending_batch(Backend::Memory).await?;
    let ops = owned.fixture.ops()?;
    let before = ops
        .load_live_head(owned.fixture.session.id())
        .await?
        .ok_or("head")?;
    scope_owner(&owned)
        .reconcile_request_completion(&scope.record().request_id)
        .await?;
    let after = ops
        .load_live_head(owned.fixture.session.id())
        .await?
        .ok_or("head")?;
    let state = crate::generated::live_request_state::decode(&after.payload.request_snapshot)?;
    let identity: CallbackBatchIdentity = serde_json::from_str(
        state
            .run_callback_records
            .get(&scope.record().run_id.to_string())
            .ok_or("identity")?,
    )?;
    let records = state
        .claim_ids
        .iter()
        .enumerate()
        .map(|(index, claim)| {
            Ok(LiveCompletionRecord {
                format: LiveLedgerFormatV1::V1,
                session_id: owned.fixture.session.id().clone(),
                channel_id: owned.source.channel_id().clone(),
                sequence: LiveObservationSeq::new(before.reference.event_count + index as u64 + 1)?,
                event: LiveCompletionEvent::CallbackSuspended {
                    claim_id: OperationId(uuid::Uuid::parse_str(claim)?),
                    request_id: scope.record().request_id.clone(),
                    input_id: scope.record().input_id.clone(),
                    run_id: scope.record().run_id.clone(),
                    batch_digest: LiveContentDigest::from_sha256(*identity.batch_digest()),
                },
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    let owner = dsl::LiveRequestMachineAuthority::recover_from_state(state)?;
    let mut exact = PreparedLiveLedgerCommit::from_request_transition_with_completions(
        owned.fixture.session.id(),
        Some(&before),
        &owner.prepare_authority(),
        records.clone(),
    )?;
    exact.quota = before.payload.used.checked_add(before.payload.reserved)?;
    assert!(
        exact
            .successor
            .payload
            .used
            .checked_add(exact.successor.payload.reserved)?
            .fits_within(exact.quota)
    );
    for corruption in 0..8 {
        let mut damaged = records.clone();
        match corruption {
            0 => {
                damaged.clear();
            }
            1 => {
                damaged.pop();
            }
            2 => {
                damaged.push(damaged[0].clone());
            }
            3 => {
                damaged[1].event = damaged[0].event.clone();
            }
            4 => {
                damaged[0].session_id = SessionId::new();
            }
            5 => {
                damaged[0].sequence = LiveObservationSeq::new(before.reference.event_count + 2)?;
            }
            6 | 7 => {
                let LiveCompletionEvent::CallbackSuspended {
                    batch_digest,
                    input_id,
                    ..
                } = &mut damaged[0].event
                else {
                    return Err("expected callback".into());
                };
                if corruption == 6 {
                    *batch_digest = LiveContentDigest::from_sha256([0; 32]);
                } else {
                    *input_id = meerkat_core::lifecycle::InputId::new();
                }
            }
            _ => unreachable!(),
        }
        assert!(
            PreparedLiveLedgerCommit::from_request_transition_with_completions(
                owned.fixture.session.id(),
                Some(&before),
                &owner.prepare_authority(),
                damaged,
            )
            .is_err(),
            "record corruption {corruption}"
        );
    }
    assert_eq!(
        ops.load_live_head(owned.fixture.session.id()).await?,
        Some(after)
    );
    Ok(())
}

#[tokio::test]
async fn native_callback_batch_commits_exact_members_and_spend_without_replay_growth() -> TestResult
{
    for backend in backends() {
        let (owned, scope) = pending_batch(backend).await?;
        let ops = owned.fixture.ops()?;
        let before = ops
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let prior = crate::generated::live_request_state::decode(&before.payload.request_snapshot)?;
        let owner = scope_owner(&owned);
        let progress = owner
            .reconcile_request_completion(&scope.record().request_id)
            .await?;
        let after = ops
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&after.payload.request_snapshot)?;
        let run = scope.record().run_id.to_string();
        assert_eq!(
            after.reference.event_count,
            before.reference.event_count + 2
        );
        assert_eq!(
            progress,
            LiveRequestCompletionProgress::CallbackSuspended(
                meerkat_core::live_observation::LiveObservationSeq::new(
                    after.reference.event_count
                )?,
            )
        );
        assert_eq!(state.run_callback_claims.get(&run), Some(&state.claim_ids));
        assert_eq!(
            state.run_callback_sequences.get(&run),
            Some(&after.reference.event_count)
        );
        assert_eq!(
            state
                .request_phases
                .get(&scope.record().request_id.to_string()),
            Some(&dsl::LiveRequestPhase::Suspended)
        );
        for claim in &state.claim_ids {
            assert_eq!(
                state.claim_credit_spent_records.get(claim),
                Some(&(prior.claim_credit_spent_records[claim] + 1))
            );
            assert!(state.claim_credit_spent_bytes[claim] > prior.claim_credit_spent_bytes[claim]);
            assert_eq!(state.claim_phases.get(claim), prior.claim_phases.get(claim));
        }
        assert!(
            after
                .payload
                .used
                .checked_add(after.payload.reserved)?
                .fits_within(before.payload.used.checked_add(before.payload.reserved)?)
        );
        assert_eq!(
            owner
                .reconcile_request_completion(&scope.record().request_id)
                .await?,
            progress
        );
        assert_eq!(
            ops.load_live_head(owned.fixture.session.id()).await?,
            Some(after.clone())
        );
        for claim in &state.claim_ids {
            let retained =
                serde_json::from_str(state.claim_records.get(claim).ok_or("claim record")?)?;
            owned
                .machine
                .settle_live_effect(
                    retained,
                    LivePhysicalEffectOutcome::Succeeded,
                    LiveCompletionText::new("late physical feedback")?,
                )
                .await?;
        }
        let late = ops
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let late_state =
            crate::generated::live_request_state::decode(&late.payload.request_snapshot)?;
        assert_eq!(late.reference.event_count, after.reference.event_count + 2);
        assert_eq!(
            late_state.run_callback_sequences,
            state.run_callback_sequences
        );
        assert_eq!(late_state.run_callback_records, state.run_callback_records);
        for claim in &state.claim_ids {
            assert_eq!(late_state.claim_credit_spent_records.get(claim), Some(&3));
        }
        assert_eq!(
            owner
                .reconcile_request_completion(&scope.record().request_id)
                .await?,
            progress
        );
        assert_eq!(
            ops.load_live_head(owned.fixture.session.id()).await?,
            Some(late)
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_callback_batch_rechecks_finalized_ordinary_row_without_partial_spend() -> TestResult
{
    for backend in backends() {
        let (owned, scope) = pending_batch(backend).await?;
        let owner = scope_owner(&owned);
        let pending = owner
            .prepare_request_completion(&scope.record().request_id)
            .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let runtime = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let mut row = owned
            .fixture
            .store
            .load_input_state(&runtime, &scope.record().input_id)
            .await?
            .ok_or("row")?;
        row.state.updated_at += chrono::Duration::seconds(1);
        owned
            .fixture
            .store
            .persist_input_state(
                &runtime,
                &InputStatePersistenceRecord::from_machine_snapshot(row)?,
            )
            .await?;
        assert!(matches!(
            owner.commit_request_completion(pending).await,
            Err(LiveRequestAuthorityError::Store(
                RuntimeStoreError::InputRowVersionConflict { .. }
            ))
        ));
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
        assert!(matches!(
            owner
                .reconcile_request_completion(&scope.record().request_id)
                .await?,
            LiveRequestCompletionProgress::CallbackSuspended(_)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn native_callback_batch_late_sqlite_failure_rolls_back_and_recovers_after_full_reopen()
-> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let (owned, scope) = pending_batch(backend).await?;
        let owner = scope_owner(&owned);
        let ops = owned.fixture.ops()?;
        let before = ops
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let connection = rusqlite::Connection::open(&owned.fixture.path)?;
        let prior_count: u64 =
            connection.query_row("SELECT count(*) FROM runtime_live_events", [], |row| {
                row.get(0)
            })?;
        connection.execute_batch(&format!(
            "CREATE TRIGGER reject_callback_second BEFORE INSERT ON runtime_live_events
             WHEN NEW.sequence = {}
             BEGIN SELECT RAISE(ABORT, 'synthetic second callback insert failure'); END;",
            before.reference.event_count + 2,
        ))?;
        assert!(
            owner
                .reconcile_request_completion(&scope.record().request_id)
                .await
                .is_err()
        );
        assert_eq!(
            ops.load_live_head(owned.fixture.session.id()).await?,
            Some(before.clone())
        );
        let after_count: u64 =
            connection.query_row("SELECT count(*) FROM runtime_live_events", [], |row| {
                row.get(0)
            })?;
        assert_eq!(
            after_count, prior_count,
            "first callback insert must also roll back"
        );
        connection.execute_batch("DROP TRIGGER reject_callback_second")?;
        drop(connection);
        let request_id = scope.record().request_id.clone();
        drop(scope);
        drop(owner);
        let OwnedFixture {
            fixture,
            machine,
            grant,
            source,
            channel,
        } = owned;
        drop(channel);
        drop(machine);
        drop(grant);
        drop(source);
        let Fixture {
            store,
            session,
            path,
            _directory,
        } = fixture;
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while Arc::strong_count(&store) != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        drop(store);
        let store: Arc<dyn RuntimeStore> = Arc::new(match backend {
            Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
            Backend::HeadCanonical => crate::store::SqliteRuntimeStore::new_head_canonical(&path)?,
            Backend::Memory => return Err("durable backend required".into()),
        });
        assert_eq!(
            store
                .live_ledger_ops()
                .ok_or("ops")?
                .load_live_head(session.id())
                .await?,
            Some(before)
        );
        let owner = crate::live_ledger::authority::store::LiveRequestStoreOwner::new(
            Arc::clone(&store),
            session.id().clone(),
        );
        assert!(matches!(
            owner.reconcile_request_completion(&request_id).await?,
            LiveRequestCompletionProgress::CallbackSuspended(_)
        ));
        let connection = rusqlite::Connection::open(&path)?;
        let count: u64 =
            connection.query_row("SELECT count(*) FROM runtime_live_events", [], |row| {
                row.get(0)
            })?;
        assert_eq!(count, prior_count + 2);
    }
    Ok(())
}
