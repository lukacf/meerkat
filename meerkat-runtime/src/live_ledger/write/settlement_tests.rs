use super::claim_tests::{read_only_observation, tool_target};
use super::restore_tests::scope_owner;
use super::*;
use crate::live_ledger::authority::store::LiveRequestAuthorityError;
use crate::live_ledger::authority::store::settlement::LiveEffectFeedback;
use crate::live_ledger::completion::{
    LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES, LiveCompletionEvent, LiveCompletionText,
    LivePhysicalEffectOutcome,
};
use crate::live_ledger::record::LiveLedgerRecord;
use crate::store::live_history::LiveHistoryReadRequest;
use meerkat_core::ToolMutationClass;
use meerkat_core::execution_scope::{
    ScopedEffectStartPermit, ScopedEffectTarget, ScopedRunAuthority,
};
use meerkat_core::ops::OperationId;

fn diagnostic() -> TestResult<LiveCompletionText<LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES>> {
    Ok(LiveCompletionText::new(
        "\0".repeat(LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES),
    )?)
}

async fn claimed(
    owned: &OwnedFixture,
) -> TestResult<(
    ScopedRunAuthority,
    ScopedEffectStartPermit<ScopedEffectTarget>,
)> {
    let scope = owned.staged_scope().await?;
    let permit = owned
        .machine
        .claim_live_effect(
            scope.clone(),
            OperationId::new(),
            tool_target("allowed_tool", ToolMutationClass::ReadOnly),
            read_only_observation()?,
        )
        .await?;
    Ok((scope, permit))
}

#[tokio::test]
async fn native_effect_feedback_appends_exact_record_and_replay_survives_later_head_advance()
-> TestResult {
    for backend in backends() {
        for outcome in LivePhysicalEffectOutcome::ALL {
            let owned = OwnedFixture::new(backend).await?;
            let (scope, permit) = claimed(&owned).await?;
            let claim = permit.claim().clone();
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            let retained: meerkat_core::execution_scope::ScopedEffectClaimRecord<
                ScopedEffectTarget,
            > = serde_json::from_str(
                crate::generated::live_request_state::decode(&before.payload.request_snapshot)?
                    .claim_records
                    .get(&claim.claim_id.as_uuid().to_string())
                    .ok_or("claim")?,
            )?;
            assert_eq!(retained, claim);
            let sequence = if *outcome == LivePhysicalEffectOutcome::NotStarted {
                assert!(
                    owned
                        .machine
                        .settle_live_effect(claim.clone(), *outcome, diagnostic()?,)
                        .await
                        .is_err()
                );
                owned
                    .machine
                    .settle_live_effect_not_started(permit.into_not_started(), diagnostic()?)
                    .await?
            } else {
                owned
                    .machine
                    .settle_live_effect(permit.into_claim(), *outcome, diagnostic()?)
                    .await?
            };
            let after = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            assert_eq!(sequence.get(), before.reference.event_count + 1);
            assert_eq!(after.reference.event_count, sequence.get());
            let window = owned
                .fixture
                .ops()?
                .read_live_history(&LiveHistoryReadRequest::new(
                    after.reference.clone(),
                    None,
                    before.reference.event_count,
                    8,
                )?)
                .await?;
            let [LiveLedgerRecord::Completion(record)] = window.records() else {
                return Err("expected one actual completion record".into());
            };
            assert_eq!(record.sequence, sequence);
            assert_eq!(record.channel_id, *owned.source.channel_id());
            assert_eq!(
                record.event,
                LiveCompletionEvent::EffectTerminal {
                    claim_id: OperationId(*claim.claim_id.as_uuid()),
                    request_id: claim.request_id.clone(),
                    outcome: *outcome,
                    token_accounting:
                        meerkat_core::execution_scope::ScopedEffectTokenAccounting::NotApplicable {},
                    diagnostic: diagnostic()?,
                }
            );
            let state =
                crate::generated::live_request_state::decode(&after.payload.request_snapshot)?;
            let key = claim.claim_id.as_uuid().to_string();
            assert_eq!(state.claim_credit_spent_records.get(&key), Some(&1));
            assert_eq!(
                state.claim_credit_spent_bytes.get(&key),
                Some(&record.encode()?.charge().encoded_bytes)
            );
            assert_eq!(
                state.claim_terminal_sequences.get(&key),
                Some(&sequence.get())
            );
            if *outcome == LivePhysicalEffectOutcome::Unknown {
                assert_eq!(
                    after.payload.used.checked_add(after.payload.reserved)?,
                    before.payload.used.checked_add(before.payload.reserved)?
                );
                assert!(after.payload.reserved.records > 0);
            } else {
                assert!(after.payload.reserved.records < before.payload.reserved.records);
            }
            owned
                .machine
                .restore_live_run_scope(scope.scope_id(), scope.record().clone())
                .await?;
            let advanced = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?;
            assert_eq!(
                owned
                    .machine
                    .settle_live_effect(claim.clone(), *outcome, diagnostic()?)
                    .await?,
                sequence
            );
            assert_eq!(
                owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?,
                advanced
            );
            assert!(
                owned
                    .machine
                    .settle_live_effect(
                        claim,
                        *outcome,
                        LiveCompletionText::new("conflicting diagnostic")?,
                    )
                    .await
                    .is_err()
            );
            assert_eq!(
                owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?,
                advanced
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_effect_feedback_cannot_replace_any_field_of_the_retained_claim() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (_, permit) = claimed(&owned).await?;
        let original = permit.into_claim();
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        for index in 0..14 {
            let mut claim = original.clone();
            match index {
                0 => claim.effect_id = OperationId::new(),
                1 => claim.request_id = OperationId::new(),
                2 => {
                    claim.claim_id = meerkat_core::execution_scope::ScopedEffectClaimId::from_uuid(
                        uuid::Uuid::new_v4(),
                    );
                }
                3 => {
                    claim.scope_id = meerkat_core::execution_scope::RunEffectScopeId::from_uuid(
                        uuid::Uuid::new_v4(),
                    );
                }
                4 => claim.input_id = meerkat_core::lifecycle::InputId::new(),
                5 => claim.run_id = meerkat_core::lifecycle::RunId::new(),
                6 => claim.executor.binding_generation += 1,
                7 => claim.executor.session_id = SessionId::new(),
                8 => claim.grant.generation = std::num::NonZeroU64::new(2).ok_or("generation")?,
                9 => {
                    claim.candidate_policy_revision =
                        meerkat_core::execution_scope::ScopedEffectPolicyRevision::TrustedHost {
                            ordinary_policy: meerkat_core::ToolExecutionPolicy::unrestricted()
                                .content_digest()?,
                            revision: std::num::NonZeroU64::new(8).ok_or("policy revision")?,
                        };
                }
                10 => claim.commit.digest[0] ^= 1,
                11 => {
                    claim.target = ScopedEffectTarget::ModelComputation {
                        request_id: meerkat_core::ops::OperationId::new(),
                        attempt: 0,
                        invocation_digest: [41; 32],
                    }
                }
                12 => {
                    claim.commit.revision =
                        std::num::NonZeroU64::new(claim.commit.revision.get() + 1)
                            .ok_or("revision")?;
                }
                13 => {
                    claim.grant.id = meerkat_core::execution_scope::ExecutionGrantId::from_uuid(
                        uuid::Uuid::new_v4(),
                    );
                }
                _ => return Err("unexpected claim mutation case".into()),
            }
            assert!(
                owned
                    .machine
                    .settle_live_effect(claim, LivePhysicalEffectOutcome::Succeeded, diagnostic()?,)
                    .await
                    .is_err(),
                "field {index}"
            );
            assert_eq!(
                owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?,
                before
            );
        }
        owned
            .machine
            .settle_live_effect(
                original,
                LivePhysicalEffectOutcome::Succeeded,
                diagnostic()?,
            )
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn native_effect_unknown_accepts_late_feedback_after_revoke_and_runtime_stop_but_not_not_started()
-> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (scope, permit) = claimed(&owned).await?;
        let claim = permit.claim().clone();
        let unknown = owned
            .machine
            .settle_live_effect(
                claim.clone(),
                LivePhysicalEffectOutcome::Unknown,
                diagnostic()?,
            )
            .await?;
        let owner = scope_owner(&owned);
        owner
            .commit(
                dsl::LiveRequestInput::Revoke {
                    grant_id: scope.record().grant.id.as_uuid().to_string(),
                    generation: scope.record().grant.generation.get(),
                },
                current_fence(),
            )
            .await?;
        owned
            .machine
            .stop_runtime_executor(owned.fixture.session.id(), "late effect feedback")
            .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(
            owned
                .machine
                .settle_live_effect_not_started(permit.into_not_started(), diagnostic()?,)
                .await
                .is_err()
        );
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
        let cold = MeerkatMachine::persistent(
            Arc::clone(&owned.fixture.store),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        let known = cold
            .settle_live_effect(
                claim.clone(),
                LivePhysicalEffectOutcome::Succeeded,
                diagnostic()?,
            )
            .await?;
        assert_eq!(known.get(), unknown.get() + 1);
        let settled = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert_eq!(
            cold.settle_live_effect(claim, LivePhysicalEffectOutcome::Succeeded, diagnostic()?)
                .await?,
            known
        );
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            settled
        );
        assert!(
            owned
                .machine
                .claim_live_effect(
                    scope,
                    OperationId::new(),
                    tool_target("allowed_tool", ToolMutationClass::ReadOnly),
                    read_only_observation()?,
                )
                .await
                .is_err()
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_effect_feedback_prepared_replay_does_not_append_twice_and_is_session_bound()
-> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (_, permit) = claimed(&owned).await?;
        let claim = permit.into_claim();
        let owner = scope_owner(&owned);
        let first = owner
            .prepare_effect_settlement(
                LiveEffectFeedback::Observed {
                    claim: claim.clone(),
                    outcome: LivePhysicalEffectOutcome::Succeeded,
                    token_accounting:
                        meerkat_core::execution_scope::ScopedEffectTokenAccounting::NotApplicable {},
                },
                diagnostic()?,
            )
            .await?;
        let duplicate = owner
            .prepare_effect_settlement(
                LiveEffectFeedback::Observed {
                    claim: claim.clone(),
                    outcome: LivePhysicalEffectOutcome::Succeeded,
                    token_accounting:
                        meerkat_core::execution_scope::ScopedEffectTokenAccounting::NotApplicable {},
                },
                diagnostic()?,
            )
            .await?;
        let wrong_owner = crate::live_ledger::authority::store::LiveRequestStoreOwner::new(
            Arc::clone(&owned.fixture.store),
            SessionId::new(),
        );
        let wrong = owner
            .prepare_effect_settlement(
                LiveEffectFeedback::Observed {
                    claim,
                    outcome: LivePhysicalEffectOutcome::Succeeded,
                    token_accounting:
                        meerkat_core::execution_scope::ScopedEffectTokenAccounting::NotApplicable {},
                },
                diagnostic()?,
            )
            .await?;
        assert!(matches!(
            wrong_owner.commit_effect_settlement(wrong).await,
            Err(LiveRequestAuthorityError::SessionMismatch)
        ));
        let sequence = owner.commit_effect_settlement(first).await?;
        let settled = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert_eq!(owner.commit_effect_settlement(duplicate).await?, sequence);
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            settled
        );
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn native_effect_feedback_late_sqlite_failure_rolls_back_record_and_spend_then_reopens()
-> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let owned = OwnedFixture::new(backend).await?;
        let (_, permit) = claimed(&owned).await?;
        let claim = permit.into_claim();
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let connection = rusqlite::Connection::open(&owned.fixture.path)?;
        connection.execute_batch(
            "CREATE TRIGGER reject_effect_feedback BEFORE UPDATE ON runtime_live_heads
             BEGIN SELECT RAISE(ABORT, 'synthetic effect feedback failure'); END;",
        )?;
        assert!(
            owned
                .machine
                .settle_live_effect(
                    claim.clone(),
                    LivePhysicalEffectOutcome::Unknown,
                    diagnostic()?,
                )
                .await
                .is_err()
        );
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
        connection.execute_batch("DROP TRIGGER reject_effect_feedback;")?;
        drop(connection);
        let unknown = owned
            .machine
            .settle_live_effect(
                claim.clone(),
                LivePhysicalEffectOutcome::Unknown,
                diagnostic()?,
            )
            .await?;
        let OwnedFixture {
            fixture,
            machine,
            channel,
            ..
        } = owned;
        drop(channel);
        drop(machine);
        let Fixture {
            store,
            session,
            path,
            _directory,
        } = fixture;
        drop(store);
        let reopened: Arc<dyn RuntimeStore> = Arc::new(match backend {
            Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
            Backend::HeadCanonical => crate::store::SqliteRuntimeStore::new_head_canonical(&path)?,
            Backend::Memory => return Err("expected SQLite".into()),
        });
        let cold = MeerkatMachine::persistent(
            Arc::clone(&reopened),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        assert_eq!(
            cold.settle_live_effect(
                claim.clone(),
                LivePhysicalEffectOutcome::Unknown,
                diagnostic()?
            )
            .await?,
            unknown
        );
        let known = cold
            .settle_live_effect(claim, LivePhysicalEffectOutcome::Failed, diagnostic()?)
            .await?;
        assert_eq!(known.get(), unknown.get() + 1);
        let head = reopened
            .live_ledger_ops()
            .ok_or("Live ops")?
            .load_live_head(session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(head.reference.event_count, known.get());
        drop(cold);
        drop(reopened);
        drop(_directory);
    }
    Ok(())
}
