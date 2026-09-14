use super::*;
use crate::live_ledger::authority::store::{LiveRequestAuthorityError, LiveRequestStoreOwner};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) fn scope_owner(owned: &OwnedFixture) -> LiveRequestStoreOwner {
    LiveRequestStoreOwner::new(
        Arc::clone(&owned.fixture.store),
        owned.fixture.session.id().clone(),
    )
}

#[tokio::test]
async fn scope_input_comparison_digest_binds_identity_and_version_and_refuses_mutation_mix()
-> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let _scope = owned.staged_scope().await?;
        let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let (mut rows, _, _) = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?
            .into_parts();
        let (row, digest) = rows.pop().ok_or("input")?;
        let input = crate::store::ExactInputStateObservation::from_exact_stored_row(row, digest)?;
        let crate::store::MachineLifecycleObservation::Decoded { version, .. } = owned
            .fixture
            .store
            .observe_machine_lifecycle(&runtime_id)
            .await?
        else {
            return Err("lifecycle".into());
        };
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let change = prepared(owned.fixture.session.id(), head.as_ref(), vec![])?
            .with_execution_fence(&input, version)?;
        let encoded = change.encoded_records()?;
        let original = change.operation_digest(&encoded)?;
        let mut other = copy_prepared(&change);
        other.input_read_fences.first_mut().ok_or("fence")?.input_id =
            meerkat_core::lifecycle::InputId::new();
        assert_ne!(other.operation_digest(&encoded)?, original);
        let mut other = copy_prepared(&change);
        other
            .input_read_fences
            .first_mut()
            .ok_or("fence")?
            .expected_row_digest = format!("sha256:{}", "0".repeat(64));
        assert_ne!(other.operation_digest(&encoded)?, original);
        let mut other = copy_prepared(&change);
        other.expected_lifecycle = None;
        assert!(other.encoded_records().is_err());
        let mut other = copy_prepared(&change);
        other.input_admission = Some(
            crate::input_state::InputStatePersistenceRecord::from_machine_snapshot(
                input.state().clone(),
            )?,
        );
        assert!(other.encoded_records().is_err());
        assert!(
            !LiveLedgerWriteProfile::AtomicHeadEventsSourcesLifecycleAdmissionStage
                .supports_execution_fence()
        );
        assert!(
            owned
                .fixture
                .ops()?
                .ledger_write_profile()
                .supports_execution_fence()
        );
    }
    Ok(())
}

#[tokio::test]
async fn scope_restoration_refuses_callback_target_grafted_onto_original_run() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let mut record = scope.record().clone();
        record.callback_continuation = Some(
            meerkat_core::execution_scope::ScopedCallbackContinuationRecord {
                target: serde_json::from_value(serde_json::json!({
                    "session_id": owned.fixture.session.id(),
                    "run_id": meerkat_core::lifecycle::RunId::new(),
                    "execution_scope": meerkat_core::execution_scope::RunEffectScopeId::from_uuid(
                        uuid::Uuid::new_v4(),
                    ),
                    "execution_boundary": meerkat_core::ops::OperationId::new(),
                    "batch_digest": vec![255; 32],
                }))?,
                results_digest: [254; 32],
            },
        );
        assert!(matches!(
            scope_owner(&owned)
                .prepare_scope_restoration(scope.scope_id(), record)
                .await,
            Err(LiveRequestAuthorityError::ScopeNotCurrent(
                "root run cannot carry a callback continuation"
            ))
        ));
        assert_eq!(
            scope_owner(&owned)
                .restore_run_scope(scope.scope_id(), scope.record().clone())
                .await?,
            scope,
        );
    }
    Ok(())
}

#[tokio::test]
async fn scope_restoration_refuses_after_actual_runtime_stop() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let owner = scope_owner(&owned);
        let pending = owner
            .prepare_scope_restoration(scope.scope_id(), scope.record().clone())
            .await?;
        owned
            .machine
            .stop_runtime_executor(owned.fixture.session.id(), "scope restoration stop race")
            .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(matches!(
            owner.commit_scope_restoration(pending).await,
            Err(LiveRequestAuthorityError::Store(
                RuntimeStoreError::InputRowVersionConflict { .. }
                    | RuntimeStoreError::MachineLifecycleVersionConflict { .. }
            ))
        ));
        assert!(
            owner
                .restore_run_scope(scope.scope_id(), scope.record().clone())
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
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn scope_restoration_late_sqlite_failure_does_not_issue_a_handle_or_rewrite_predecessors()
-> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let inputs = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?;
        let lifecycle = owned
            .fixture
            .store
            .observe_machine_lifecycle(&runtime_id)
            .await?;
        let connection = rusqlite::Connection::open(&owned.fixture.path)?;
        connection.execute_batch(
            "CREATE TRIGGER reject_restored_scope BEFORE UPDATE ON runtime_live_heads
             BEGIN SELECT RAISE(ABORT, 'synthetic scope restoration failure'); END;",
        )?;
        assert!(matches!(
            scope_owner(&owned)
                .restore_run_scope(scope.scope_id(), scope.record().clone())
                .await,
            Err(LiveRequestAuthorityError::Store(
                RuntimeStoreError::WriteFailed(_)
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
        let after_inputs = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?;
        assert_eq!(after_inputs.exact_set_token(), inputs.exact_set_token());
        assert_eq!(
            after_inputs.input_set_revision(),
            inputs.input_set_revision()
        );
        assert_eq!(
            owned
                .fixture
                .store
                .observe_machine_lifecycle(&runtime_id)
                .await?,
            lifecycle
        );
        connection.execute_batch("DROP TRIGGER reject_restored_scope;")?;
        assert_eq!(
            scope_owner(&owned)
                .restore_run_scope(scope.scope_id(), scope.record().clone())
                .await?,
            scope
        );
    }
    Ok(())
}

#[tokio::test]
async fn scope_restoration_seals_current_scope_without_rewriting_input_source_or_lifecycle()
-> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let inputs = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?;
        let lifecycle = owned
            .fixture
            .store
            .observe_machine_lifecycle(&runtime_id)
            .await?;
        let source = owned
            .fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let restored = owned
            .machine
            .restore_live_run_scope(
                scope.scope_id(),
                serde_json::from_slice(&serde_json::to_vec(scope.record())?)?,
            )
            .await?;
        assert_eq!(restored, scope);
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(after.reference.revision, before.reference.revision + 1);
        assert_eq!(after.payload, before.payload);
        let after_inputs = owned
            .fixture
            .store
            .load_input_states_with_versions(&runtime_id)
            .await?;
        assert_eq!(after_inputs.exact_set_token(), inputs.exact_set_token());
        assert_eq!(
            after_inputs.input_set_revision(),
            inputs.input_set_revision()
        );
        assert_eq!(
            owned
                .fixture
                .store
                .observe_machine_lifecycle(&runtime_id)
                .await?,
            lifecycle
        );
        let after_source = owned
            .fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        assert_eq!(after_source.bytes(), source.bytes());
        assert_eq!(after_source.digest(), source.digest());
    }
    Ok(())
}

#[tokio::test]
async fn scope_restoration_refuses_forged_scope_content_without_advancing_head() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let mut forged = scope.record().clone();
        forged.remaining.model_computations += 1;
        assert!(
            owned
                .machine
                .restore_live_run_scope(scope.scope_id(), forged)
                .await
                .is_err()
        );
        let mut forged = scope.record().clone();
        forged.run_id = meerkat_core::lifecycle::RunId::new();
        assert!(
            owned
                .machine
                .restore_live_run_scope(scope.scope_id(), forged)
                .await
                .is_err()
        );
        assert!(owned.machine.restore_live_run_scope(
            meerkat_core::execution_scope::RunEffectScopeId::from_uuid(uuid::Uuid::new_v4()),
            scope.record().clone(),
        ).await.is_err());
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
    }
    Ok(())
}

#[tokio::test]
async fn scope_restoration_checks_input_on_new_commit_and_replay_without_minting_twice()
-> TestResult {
    for backend in backends() {
        for replay in [false, true] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let owner = scope_owner(&owned);
            let pending = owner
                .prepare_scope_restoration(scope.scope_id(), scope.record().clone())
                .await?;
            if replay {
                let duplicate = owner
                    .prepare_scope_restoration(scope.scope_id(), scope.record().clone())
                    .await?;
                let repeated = owner
                    .prepare_scope_restoration(scope.scope_id(), scope.record().clone())
                    .await?;
                assert_eq!(owner.commit_scope_restoration(duplicate).await?, scope);
                assert!(matches!(owner.commit_scope_restoration(repeated).await,
                    Err(LiveRequestAuthorityError::NotNewlyCommitted(outcome))
                        if matches!(*outcome, LiveLedgerCommitOutcome::AlreadyCommitted { .. })));
            }
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?;
            let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
            let (mut rows, _, _) = owned
                .fixture
                .store
                .load_input_states_with_versions(&runtime_id)
                .await?
                .into_parts();
            let (mut row, digest) = rows.pop().ok_or("input")?;
            assert!(rows.is_empty());
            // Inject an intervening, different durable run binding.
            row.seed.last_run_id = Some(meerkat_core::lifecycle::RunId::new());
            let replacement =
                crate::input_state::InputStatePersistenceRecord::from_machine_snapshot(row)?
                    .with_expected_row_digest(digest);
            owned
                .fixture
                .store
                .persist_input_state(&runtime_id, &replacement)
                .await?;
            let changed = owned
                .fixture
                .store
                .load_input_states_with_versions(&runtime_id)
                .await?;
            assert!(matches!(
                owner.commit_scope_restoration(pending).await,
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
            let after = owned
                .fixture
                .store
                .load_input_states_with_versions(&runtime_id)
                .await?;
            assert_eq!(after.exact_set_token(), changed.exact_set_token());
            assert_eq!(after.input_set_revision(), changed.input_set_revision());
        }
    }
    Ok(())
}

#[tokio::test]
async fn scope_restoration_checks_lifecycle_on_new_commit_and_replay() -> TestResult {
    for backend in backends() {
        for replay in [false, true] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let owner = scope_owner(&owned);
            let pending = owner
                .prepare_scope_restoration(scope.scope_id(), scope.record().clone())
                .await?;
            if replay {
                owner
                    .restore_run_scope(scope.scope_id(), scope.record().clone())
                    .await?;
            }
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?;
            install_grant_executor(
                &owned.fixture,
                &scope.record().executor.runtime_epoch,
                scope.record().executor.binding_generation,
            )
            .await?;
            assert!(matches!(
                owner.commit_scope_restoration(pending).await,
                Err(LiveRequestAuthorityError::Store(
                    RuntimeStoreError::MachineLifecycleVersionConflict { .. }
                ))
            ));
            assert!(matches!(
                owner
                    .restore_run_scope(scope.scope_id(), scope.record().clone())
                    .await,
                Err(LiveRequestAuthorityError::ScopeNotCurrent(_))
            ));
            assert_eq!(
                owned
                    .fixture
                    .ops()?
                    .load_live_head(owned.fixture.session.id())
                    .await?,
                before
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn scope_restoration_rechecks_expiry_at_publication_and_revocation_after_prepare()
-> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let clock = Arc::new(AtomicU64::new(0));
        let observed_clock = Arc::clone(&clock);
        let owner = scope_owner(&owned)
            .with_clock(Arc::new(move || Ok(observed_clock.load(Ordering::SeqCst))));
        let pending = owner
            .prepare_scope_restoration(scope.scope_id(), scope.record().clone())
            .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        clock.store(u64::MAX, Ordering::SeqCst);
        assert!(matches!(owner.commit_scope_restoration(pending).await,
            Err(LiveRequestAuthorityError::Store(RuntimeStoreError::LiveRequestPublicationRejected { reason }))
                if reason.contains("changed before publication")));
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
        clock.store(0, Ordering::SeqCst);
        let pending = owner
            .prepare_scope_restoration(scope.scope_id(), scope.record().clone())
            .await?;
        owner
            .commit(
                dsl::LiveRequestInput::Revoke {
                    grant_id: scope.record().grant.id.as_uuid().to_string(),
                    generation: scope.record().grant.generation.get(),
                },
                current_fence(),
            )
            .await?;
        let revoked = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(matches!(owner.commit_scope_restoration(pending).await,
            Err(LiveRequestAuthorityError::NotNewlyCommitted(outcome))
                if matches!(*outcome, LiveLedgerCommitOutcome::Conflict { .. })));
        assert!(matches!(
            owner
                .restore_run_scope(scope.scope_id(), scope.record().clone())
                .await,
            Err(LiveRequestAuthorityError::Transition(_))
        ));
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            revoked
        );
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn scope_restoration_reopens_both_sqlite_profiles_and_reseals_only_current_run() -> TestResult
{
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let OwnedFixture {
            fixture,
            machine,
            grant,
            source: _,
            channel,
        } = owned;
        drop(channel);
        drop(machine);
        drop(grant);
        let Fixture {
            store,
            session,
            _directory,
            path,
        } = fixture;
        drop(store);
        let store: Arc<dyn RuntimeStore> = Arc::new(match backend {
            Backend::WholeBlob => crate::store::SqliteRuntimeStore::new_whole_blob(&path)?,
            Backend::HeadCanonical => crate::store::SqliteRuntimeStore::new_head_canonical(&path)?,
            Backend::Memory => return Err("expected SQLite".into()),
        });
        let owner = LiveRequestStoreOwner::new(Arc::clone(&store), session.id().clone());
        assert_eq!(
            owner
                .restore_run_scope(scope.scope_id(), scope.record().clone())
                .await?,
            scope
        );
        drop(owner);
        drop(store);
        drop(_directory);
    }
    Ok(())
}
