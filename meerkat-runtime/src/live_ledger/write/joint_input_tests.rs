use super::*;
use crate::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, LiveRequestInput,
};
use crate::input_state::{
    InputLifecycleState, InputState, InputStatePersistenceRecord, InputStateSeed, PolicySnapshot,
    StoredInputState,
};
use crate::live_ledger::source::LiveSourceRow;
use crate::live_request::LiveExecutionRequestRecord;
use crate::live_source::LiveSourceEntryRecord;
use meerkat_core::lifecycle::InputId;
use meerkat_core::live_execution::evidence::DelegatedRequestProvenance;
use meerkat_core::types::HandlingMode;

#[tokio::test]
async fn archive_storage_purpose_and_missing_lifecycle_cannot_admit_work() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let mut missing = joint_input_fixture(&fixture).await?;
        missing.expected_lifecycle = Some(crate::store::MachineLifecycleExpectedVersion::Missing);
        assert!(missing.validate_input_admission().is_err());
        let before = fixture.ops()?.load_live_head(fixture.session.id()).await?;
        let mut archive = copy_prepared(&missing);
        archive.purpose = LiveLedgerWritePurpose::ArchiveIngressFence;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(archive, current_fence())
                .await
                .is_err()
        );
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            before
        );
        assert!(
            fixture
                .store
                .load_input_states(&LogicalRuntimeId::for_session(fixture.session.id()))
                .await?
                .is_empty()
        );
    }
    Ok(())
}

#[tokio::test]
async fn joint_live_input_materialization_cannot_launder_a_bare_unscoped_primitive() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let change = joint_input_fixture(&fixture).await?;
        let bundle = change.input_admission().ok_or("input")?.clone_stored();
        fixture
            .ops()?
            .commit_live_ledger(change, current_fence())
            .await?;
        let input = bundle.state.persisted_input.ok_or("payload")?;
        let Input::LiveRequest(live) = &input else {
            return Err("Live input".into());
        };
        let mut projection = crate::input::runtime_input_projection_for_machine_batch(&input);
        projection.append = live
            .request
            .materialize(fixture.store.as_ref(), &live.header.id)
            .await?;
        assert!(projection.append.is_some());
        projection.deferred_live_request = None;
        let error = crate::runtime_loop::try_projected_inputs_to_primitive_with_boundary(
            &[(bundle.state.input_id, input)],
            &[projection],
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
            &[bundle.state.runtime_semantics.ok_or("semantics")?],
        )
        .expect_err("materialized text is not a scoped execution handoff");
        assert_eq!(error.field, "execution_scope");
    }
    Ok(())
}

// Synthetic owner delta: these tests qualify the physical joint transaction,
// not a production source reservation or ordinary-admission producer.
async fn joint_input_fixture(fixture: &Fixture) -> TestResult<PreparedLiveLedgerCommit> {
    let epoch = meerkat_core::RuntimeEpochId::new();
    install_grant_executor(fixture, &epoch, 1).await?;
    fixture
        .ops()?
        .commit_live_ledger(
            prepared(fixture.session.id(), None, vec![observation(1, "initial")?])?,
            current_fence(),
        )
        .await?;
    let source = reserved_source(fixture, "joint-input").await?;
    let before = fixture
        .ops()?
        .load_live_head(fixture.session.id())
        .await?
        .ok_or("head")?;
    let mut reserve = prepared(fixture.session.id(), Some(&before), vec![])?;
    reserve.expected_actor = Some(fixture.actor().await?);
    add_source(&mut reserve, None, source.clone())?;
    fixture
        .ops()?
        .commit_live_ledger(reserve, current_fence())
        .await?;
    let before = fixture
        .ops()?
        .load_live_head(fixture.session.id())
        .await?
        .ok_or("head")?;
    let LiveSourceEntryRecord::Reservation { record } = source.record()? else {
        return Err("reservation".into());
    };
    let evidence = record.frozen_request().ok_or("evidence")?;
    let input_id = InputId::new();
    let input = Input::LiveRequest(LiveRequestInput {
        header: InputHeader {
            id: input_id.clone(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::LiveRequest,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: Some(crate::identifiers::IdempotencyKey(format!(
                "joint-{input_id}"
            ))),
            supersession_key: None,
            correlation_id: None,
        },
        request: LiveExecutionRequestRecord::LiveRequest {
            provenance: DelegatedRequestProvenance::new(
                record.request_id().clone(),
                record.source().clone(),
                evidence.kind(),
                evidence.request().digest(),
            )?,
            source_row: record.frozen_digest()?,
        },
    });
    let projection = crate::policy_table::generated_admission_projection_for_input(&input, true)?;
    let mut state = InputState::new_accepted(input_id.clone());
    state.runtime_semantics = Some(projection.runtime_semantics);
    state.policy = Some(PolicySnapshot {
        version: crate::policy_table::generated_default_policy_version(),
        decision: projection.policy,
    });
    state.durability = Some(InputDurability::Durable);
    state.idempotency_key = input.header().idempotency_key.clone();
    state.persisted_input = Some(input);
    let input = InputStatePersistenceRecord::from_machine_snapshot(StoredInputState {
        state,
        seed: InputStateSeed {
            phase: InputLifecycleState::Queued,
            last_run_id: None,
            last_boundary_sequence: None,
            admission_sequence: Some(1),
            terminal_outcome: None,
            attempt_count: 0,
            recovery_lane: Some(HandlingMode::Queue),
        },
    })?;
    let mut image = serde_json::to_value(source.record()?)?;
    image["record"]["disposition"] = serde_json::json!({
        "kind": "admitted", "receipt": {
            "source": record.source(), "input_id": input_id,
            "executor": { "session_id": fixture.session.id(), "realm": "owner",
                "runtime_epoch": epoch, "binding_generation": 1 },
            "grant": record.grant().ok_or("grant")?,
            "ingress_generation_at_admission": 1,
            "commit": { "revision": before.reference.revision + 1, "digest": vec![1; 32] }
        }
    });
    let replacement = LiveSourceRow::encode(&serde_json::from_value(image)?)?;
    let mut change = prepared(
        fixture.session.id(),
        Some(&before),
        vec![observation(2, "admission")?],
    )?;
    let crate::store::MachineLifecycleObservation::Decoded { version, .. } = fixture
        .store
        .observe_machine_lifecycle(&LogicalRuntimeId::for_session(fixture.session.id()))
        .await?
    else {
        return Err("lifecycle".into());
    };
    change.expected_lifecycle = Some(crate::store::MachineLifecycleExpectedVersion::Version(
        version,
    ));
    add_source(&mut change, Some(&source), replacement)?;
    change.input_admission = Some(input);
    Ok(change)
}

#[tokio::test]
async fn joint_live_input_head_source_and_indexes_commit_and_replay_once() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        assert!(
            fixture
                .ops()?
                .ledger_write_profile()
                .supports_input_admission()
        );
        let change = joint_input_fixture(&fixture).await?;
        let runtime_id = LogicalRuntimeId::for_session(fixture.session.id());
        let expected = serde_json::to_vec(change.input_admission().ok_or("input")?.as_stored())?;
        assert!(
            fixture
                .store
                .load_input_states(&runtime_id)
                .await?
                .is_empty()
        );
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), current_fence())
                .await?,
            LiveLedgerCommitOutcome::Committed { .. }
        ));
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), current_fence())
                .await?,
            LiveLedgerCommitOutcome::AlreadyCommitted { .. }
        ));
        let inputs = fixture.store.load_input_states(&runtime_id).await?;
        let [crate::store::InputStateRow::Decoded(input)] = inputs.as_slice() else {
            return Err(format!("expected one decoded ordinary input, got {inputs:?}").into());
        };
        assert_eq!(serde_json::to_vec(input.as_ref())?, expected);
        let indexed = fixture
            .store
            .load_input_state_by_idempotency_key(
                &runtime_id,
                input.state.idempotency_key.as_ref().ok_or("key")?,
            )
            .await?
            .ok_or("missing admission index")?;
        assert_eq!(serde_json::to_vec(indexed.state())?, expected);
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            Some(change.successor().clone())
        );
        assert_eq!(
            fixture
                .ops()?
                .lookup_live_source(change.sources[0].replacement.source())
                .await?
                .ok_or("source")?
                .bytes(),
            change.sources[0].replacement.bytes()
        );
        let mut omitted = copy_prepared(&change);
        omitted.input_admission = None;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(omitted, current_fence())
                .await?,
            LiveLedgerCommitOutcome::Conflict { .. }
        ));
    }
    Ok(())
}

#[tokio::test]
async fn joint_live_input_fence_and_existing_row_leave_head_and_source_unchanged() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let change = joint_input_fixture(&fixture).await?;
        let runtime_id = LogicalRuntimeId::for_session(fixture.session.id());
        let head = fixture.ops()?.load_live_head(fixture.session.id()).await?;
        let source = fixture
            .ops()?
            .lookup_live_source(change.sources[0].replacement.source())
            .await?
            .ok_or("source")?;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(
                    copy_prepared(&change),
                    Arc::new(Fence(RuntimeStoreWriteFenceOutcome::Conflict {
                        reason: "revoked".into()
                    })),
                )
                .await,
            Err(RuntimeStoreError::WriteFenceConflict { .. })
        ));
        assert!(
            fixture
                .store
                .load_input_states(&runtime_id)
                .await?
                .is_empty()
        );
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            head
        );
        fixture
            .store
            .persist_input_state(&runtime_id, change.input_admission().ok_or("input")?)
            .await?;
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(change, current_fence())
                .await,
            Err(RuntimeStoreError::InputRowVersionConflict { .. })
        ));
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            head
        );
        assert_eq!(
            fixture
                .ops()?
                .lookup_live_source(source.source())
                .await?
                .ok_or("source")?
                .bytes(),
            source.bytes()
        );
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn joint_live_input_late_source_failure_rolls_back_input_index_and_all_live_rows()
-> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let fixture = Fixture::new(backend).await?;
        let change = joint_input_fixture(&fixture).await?;
        let runtime_id = LogicalRuntimeId::for_session(fixture.session.id());
        let key = change
            .input_admission()
            .ok_or("input")?
            .as_stored()
            .state
            .idempotency_key
            .as_ref()
            .ok_or("key")?;
        let before = fixture.ops()?.load_live_head(fixture.session.id()).await?;
        let source = fixture
            .ops()?
            .lookup_live_source(change.sources[0].replacement.source())
            .await?
            .ok_or("source")?;
        let conn = rusqlite::Connection::open(&fixture.path)?;
        conn.execute_batch(
            "CREATE TRIGGER fail_joint_source BEFORE UPDATE ON runtime_live_sources
             BEGIN SELECT RAISE(ABORT, 'late joint source failure'); END;",
        )?;
        assert!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), current_fence())
                .await
                .is_err()
        );
        assert!(
            fixture
                .store
                .load_input_states(&runtime_id)
                .await?
                .is_empty()
        );
        assert!(
            fixture
                .store
                .load_input_state_by_idempotency_key(&runtime_id, key)
                .await?
                .is_none()
        );
        assert_eq!(
            fixture.ops()?.load_live_head(fixture.session.id()).await?,
            before
        );
        assert_eq!(
            fixture
                .ops()?
                .lookup_live_source(source.source())
                .await?
                .ok_or("source")?
                .bytes(),
            source.bytes()
        );
        conn.execute_batch("DROP TRIGGER fail_joint_source;")?;
        drop(conn);
        assert!(matches!(
            fixture
                .ops()?
                .commit_live_ledger(copy_prepared(&change), current_fence())
                .await?,
            LiveLedgerCommitOutcome::Committed { .. }
        ));
        assert!(
            fixture
                .store
                .load_input_state_by_idempotency_key(&runtime_id, key)
                .await?
                .is_some()
        );
    }
    Ok(())
}
