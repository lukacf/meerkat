use super::claim_tests::{read_only_observation, tool_target};
use super::restore_tests::scope_owner;
use super::*;
use crate::RuntimeDriverError;
use crate::input_state::{InputLifecycleState, InputStatePersistenceRecord};
use crate::live_ledger::authority::dsl::LiveInputRecoveryDisposition;
use crate::live_ledger::authority::store::request_completion::LiveRequestCompletionProgress;
use crate::live_ledger::completion::{LiveCompletionText, LivePhysicalEffectOutcome};
use crate::meerkat_machine::driver::{
    machine_apply_recovered_input_normalization, machine_normalize_recovered_input_state,
};

#[tokio::test]
async fn native_scoped_recovery_observes_late_feedback_without_replaying_or_completing_input()
-> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let owner = scope_owner(&owned);
        let claim = owner
            .claim_effect(
                scope.clone(),
                meerkat_core::ops::OperationId::new(),
                tool_target("allowed_tool", meerkat_core::ToolMutationClass::ReadOnly),
                read_only_observation()?,
            )
            .await?
            .into_claim();
        let request = &scope.record().request_id;
        assert_eq!(
            owner.reconcile_request_completion(request).await?,
            LiveRequestCompletionProgress::OrdinaryPending
        );
        let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let before = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?;
        let bundle = before.clone().into_parts().0.pop().ok_or("input")?.0;
        assert!(matches!(
            machine_normalize_recovered_input_state(
                owned.fixture.store.as_ref(),
                &runtime_id,
                bundle.clone(),
            )
            .await,
            Err(RuntimeDriverError::RecoveryRepairBlocked { .. })
        ));
        // Simulate loss of durable run residency, not an observed physical failure.
        install_grant_executor(
            &owned.fixture,
            &scope.record().executor.runtime_epoch,
            scope.record().executor.binding_generation,
        )
        .await?;
        let LiveRequestCompletionProgress::RecoveryHeld(before_feedback) =
            owner.reconcile_request_completion(request).await?
        else {
            return Err("unresolved claim did not produce a typed hold".into());
        };
        assert_eq!(
            before_feedback.disposition,
            LiveInputRecoveryDisposition::HoldOutstandingEffects
        );
        let sequence = owned
            .machine
            .settle_live_effect(
                claim.clone(),
                LivePhysicalEffectOutcome::Succeeded,
                LiveCompletionText::new("retained effect completion")?,
            )
            .await?;
        let LiveRequestCompletionProgress::RecoveryHeld(after_feedback) =
            owner.reconcile_request_completion(request).await?
        else {
            return Err("effect feedback fabricated request completion".into());
        };
        assert_eq!(
            after_feedback.disposition,
            LiveInputRecoveryDisposition::HoldUnresolvedRun
        );
        assert_ne!(
            before_feedback.evidence_digest,
            after_feedback.evidence_digest
        );
        assert_eq!(
            owned
                .machine
                .settle_live_effect(
                    claim,
                    LivePhysicalEffectOutcome::Succeeded,
                    LiveCompletionText::new("retained effect completion")?,
                )
                .await?,
            sequence
        );
        assert_eq!(
            owner.reconcile_request_completion(request).await?,
            LiveRequestCompletionProgress::RecoveryHeld(after_feedback)
        );
        let after = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?;
        assert_eq!(before.exact_set_token(), after.exact_set_token());
        assert_eq!(before.input_set_revision(), after.input_set_revision());
        assert_eq!(bundle.seed.phase, InputLifecycleState::Staged);
        assert_eq!(
            bundle.seed.last_run_id.as_ref(),
            Some(&scope.record().run_id)
        );
        assert!(bundle.state.terminal_completion.is_none());
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(
            crate::generated::live_request_state::decode(&head.payload.request_snapshot)?
                .request_phases
                .get(&request.to_string()),
            Some(&crate::live_ledger::authority::dsl::LiveRequestPhase::Running)
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_recovery_rejects_lost_payload_and_nonmatching_run() -> TestResult {
    #[derive(Debug, Clone, Copy)]
    enum Fault {
        MissingRun,
        ForeignRun,
        MissingPayload,
    }
    for backend in backends() {
        for fault in [Fault::MissingRun, Fault::ForeignRun, Fault::MissingPayload] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
            let mut bundle = owned
                .fixture
                .store
                .load_input_state(&runtime_id, &scope.record().input_id)
                .await?
                .ok_or("input")?;
            match fault {
                Fault::MissingRun => {
                    bundle.seed.phase = InputLifecycleState::Queued;
                    bundle.seed.last_run_id = None;
                    bundle.seed.last_boundary_sequence = None;
                }
                Fault::ForeignRun => {
                    bundle.seed.last_run_id = Some(meerkat_core::lifecycle::RunId::new());
                }
                Fault::MissingPayload => bundle.state.persisted_input = None,
            }
            owned
                .fixture
                .store
                .persist_input_state(
                    &runtime_id,
                    &InputStatePersistenceRecord::from_machine_snapshot(bundle.clone())?,
                )
                .await?;
            let before = owned
                .fixture
                .store
                .load_input_states_with_versions(&runtime_id)
                .await?;
            let head = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?;
            let error = machine_normalize_recovered_input_state(
                owned.fixture.store.as_ref(),
                &runtime_id,
                bundle.clone(),
            )
            .await
            .err()
            .ok_or("corrupt scoped input was normalized")?;
            assert!(
                matches!(error, RuntimeDriverError::RecoveryCorruption { .. }),
                "{backend:?}/{fault:?}: {error:?}"
            );
            if matches!(fault, Fault::MissingPayload) {
                let image = serde_json::to_value(&bundle)?;
                assert!(matches!(
                    machine_apply_recovered_input_normalization(&mut bundle, None),
                    Err(RuntimeDriverError::RecoveryCorruption { .. })
                ));
                assert_eq!(serde_json::to_value(&bundle)?, image);
            }
            let after = owned
                .fixture
                .store
                .load_input_states_with_versions(&runtime_id)
                .await?;
            assert_eq!(before.exact_set_token(), after.exact_set_token());
            assert_eq!(before.input_set_revision(), after.input_set_revision());
            assert_eq!(
                owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?,
                head
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_recovery_keeps_exact_staged_input_and_outstanding_claims() -> TestResult {
    for backend in backends() {
        for claimed in [false, true] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            if claimed {
                let _claim = scope_owner(&owned)
                    .claim_effect(
                        scope.clone(),
                        meerkat_core::ops::OperationId::new(),
                        tool_target("allowed_tool", meerkat_core::ToolMutationClass::ReadOnly),
                        read_only_observation()?,
                    )
                    .await?;
            }
            let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
            let before = owned
                .fixture
                .store
                .load_input_states_with_versions(&runtime_id)
                .await?;
            let bundle = before.clone().into_parts().0.pop().ok_or("input")?.0;
            let image = serde_json::to_value(&bundle)?;
            let head = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?;
            let lifecycle = owned
                .fixture
                .store
                .observe_machine_lifecycle(&runtime_id)
                .await?;
            let error = machine_normalize_recovered_input_state(
                owned.fixture.store.as_ref(),
                &runtime_id,
                bundle.clone(),
            )
            .await
            .err()
            .ok_or("scoped input was normalized")?;
            assert!(
                matches!(error,
                    RuntimeDriverError::RecoveryRepairBlocked { evidence_digest: Some(_), ref reason }
                    if reason.contains(if claimed { "HoldOutstandingEffects" } else { "HoldUnresolvedRun" })
                ),
                "{backend:?}: {error:?}"
            );
            let mut direct = bundle;
            assert!(matches!(
                machine_apply_recovered_input_normalization(&mut direct, None),
                Err(RuntimeDriverError::RecoveryRepairBlocked { .. })
            ));
            assert_eq!(serde_json::to_value(&direct)?, image);
            let after = owned
                .fixture
                .store
                .load_input_states_with_versions(&runtime_id)
                .await?;
            assert_eq!(before.exact_set_token(), after.exact_set_token());
            assert_eq!(before.input_set_revision(), after.input_set_revision());
            assert_eq!(
                owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?,
                head
            );
            assert_eq!(
                owned
                    .fixture
                    .store
                    .observe_machine_lifecycle(&runtime_id)
                    .await?,
                lifecycle
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_scoped_recovery_accepts_unbound_admission_and_refuses_stale_snapshot() -> TestResult
{
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (admitted, _completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let bundle = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?
            .into_parts()
            .0
            .pop()
            .ok_or("input")?
            .0;
        let (normalized, _) = machine_normalize_recovered_input_state(
            owned.fixture.store.as_ref(),
            &runtime_id,
            bundle.clone(),
        )
        .await?;
        assert_eq!(normalized.seed.phase, InputLifecycleState::Queued);
        assert_eq!(normalized.seed.last_run_id, None);
        owned
            .machine
            .prepare_next_batch_for_live_scope_authority_test(
                owned.fixture.session.id(),
                admitted.record().input_id(),
            )
            .await?;
        assert!(matches!(
            machine_normalize_recovered_input_state(
                owned.fixture.store.as_ref(),
                &runtime_id,
                bundle,
            )
            .await,
            Err(RuntimeDriverError::RecoveryBackoff { .. })
        ));
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn native_scoped_recovery_holds_after_full_sqlite_close_without_reminting() -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let _claim = scope_owner(&owned)
            .claim_effect(
                scope.clone(),
                meerkat_core::ops::OperationId::new(),
                tool_target("allowed_tool", meerkat_core::ToolMutationClass::ReadOnly),
                read_only_observation()?,
            )
            .await?;
        let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let before = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?;
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let OwnedFixture {
            fixture,
            machine,
            grant,
            channel,
            ..
        } = owned;
        drop(channel);
        drop(machine);
        drop(grant);
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
            Backend::Memory => unreachable!(),
        });
        let machine = MeerkatMachine::persistent(
            Arc::clone(&reopened),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        let error = machine
            .prepare_bindings(session.id().clone())
            .await
            .err()
            .ok_or("cold scoped run was made runnable")?;
        assert!(
            matches!(
                error,
                crate::meerkat_machine::RuntimeBindingsError::RecoveryHeld {
                    evidence_digest: Some(_),
                    ..
                }
            ),
            "{backend:?}: {error:?}"
        );
        let after = reopened
            .load_input_states_with_versions(&runtime_id)
            .await?;
        assert_eq!(before.exact_set_token(), after.exact_set_token());
        assert_eq!(before.input_set_revision(), after.input_set_revision());
        assert_eq!(
            reopened
                .live_ledger_ops()
                .ok_or("ledger")?
                .load_live_head(session.id())
                .await?,
            head
        );
        drop(machine);
        drop(reopened);
        drop(_directory);
    }
    Ok(())
}
