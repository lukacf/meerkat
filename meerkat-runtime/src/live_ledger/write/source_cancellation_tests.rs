use super::*;
use crate::live_ledger::authority::store::LiveRequestStoreOwner;
use meerkat_core::live_execution::request::{
    LiveApplicationRequestId, LiveRequestCancelIntent, LiveRequestCancellationReason,
    LiveSourceIdentity,
};

fn source_owner(owned: &OwnedFixture) -> LiveRequestStoreOwner {
    LiveRequestStoreOwner::new(
        Arc::clone(&owned.fixture.store),
        owned.fixture.session.id().clone(),
    )
}

#[tokio::test]
async fn native_reserved_source_is_funded_before_ordinary_admission() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("reserved head")?;
        let state = crate::generated::live_request_state::decode(&before.payload.request_snapshot)?;
        assert!(state.admitted_requests.is_empty());
        assert!(state.request_inputs.is_empty());
        let request = state
            .source_requests
            .get(&serde_json::to_string(&owned.source)?)
            .ok_or("source owner")?;
        let budget = crate::live_ledger::authority::store::request_credits::RequestCompletionBudget::measured()?;
        assert_eq!(
            state.request_credit_records.get(request),
            Some(&budget.envelope.total().records)
        );
        assert_eq!(
            before.payload.reserved.records,
            budget.envelope.total().records
                + crate::live_ledger::write::transcript_commit::reserved_charge(
                    &crate::generated::live_transcript_state::decode(
                        &before.payload.transcript_snapshot
                    )?,
                )?
                .records
        );
        let mut candidate = dsl::LiveRequestMachineAuthority::recover_from_state(state.clone())?
            .prepare_authority();
        let changed_budget = dsl::LiveRequestInput::Admit {
            source_ingress_open: true,
            request_id: request.clone(),
            source: state.request_sources[request].clone(),
            payload: state.request_payloads[request].clone(),
            input_id: uuid::Uuid::new_v4().to_string(),
            admission_commit: uuid::Uuid::new_v4().to_string(),
            profile_revision: state.grant_profile_revision.clone(),
            ingress_generation: before.payload.ingress_generation,
            credit_records: budget.envelope.total().records + 1,
            credit_bytes: budget.envelope.total().encoded_bytes,
            snapshot_ceiling: budget.snapshot_ceiling,
            now: u64::try_from(chrono::Utc::now().timestamp_millis())?,
        };
        assert!(dsl::LiveRequestMachineMutator::apply(&mut candidate, changed_budget).is_err());
        assert_eq!(candidate.state(), &state);
        owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("admitted head")?;
        assert_eq!(
            after.payload.reserved, before.payload.reserved,
            "admission transfers existing capacity without funding it twice"
        );
    }
    Ok(())
}

fn cancel_intent(source: LiveSourceKey) -> LiveRequestCancelIntent {
    LiveRequestCancelIntent {
        source,
        reason: LiveRequestCancellationReason::OperatorRequested,
    }
}

#[tokio::test]
async fn native_source_cancellation_precedes_reservation_and_preserves_first_intent() -> TestResult
{
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let source = LiveSourceKey::new(
            owned.source.session_id().clone(),
            owned.source.channel_id().clone(),
            LiveSourceIdentity::ApplicationRequest {
                request_id: LiveApplicationRequestId::from_uuid(uuid::Uuid::new_v4()),
            },
        )?;
        let intent = cancel_intent(source.clone());
        assert_eq!(
            owned.machine.cancel_live_request(intent.clone()).await?,
            intent
        );
        let second = LiveRequestCancelIntent {
            source: source.clone(),
            reason: LiveRequestCancellationReason::GrantRevoked,
        };
        assert_eq!(owned.machine.cancel_live_request(second).await?, intent);
        assert!(matches!(
            owned.fixture.ops()?.lookup_live_source(&source).await?.ok_or("source")?.record()?,
            LiveSourceEntryRecord::CancellationOnly { intent: retained } if retained == intent
        ));
        assert!(
            owned
                .machine
                .commit_live_input_admission(source.clone(), &owned.grant)
                .await
                .is_err()
        );
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert!(
            !state
                .source_requests
                .contains_key(&serde_json::to_string(&source)?)
        );
        assert!(state.run_requests.is_empty());
        assert!(state.claim_ids.is_empty());
        assert!(state.cancelled_requests.is_empty());
        assert!(state.ingress_open);
        assert!(!state.grant_revoked);
        let mut candidate =
            dsl::LiveRequestMachineAuthority::recover_from_state(state)?.prepare_authority();
        let grant = owned.grant.record();
        let before = candidate.state().clone();
        let mut reservation = dsl::LiveRequestInput::Reserve {
            content_complete: true,
            content_discontinuous: false,
            content_empty: false,
            content_fits: true,
            request_id: uuid::Uuid::new_v4().to_string(),
            source: serde_json::to_string(&source)?,
            payload: "payload".into(),
            evidence:
                meerkat_core::live_execution::request::LiveRequestEvidenceKind::ApplicationSnapshot,
            profile_revision: serde_json::to_string(&grant.declaration().profile_revision)?,
            parent_scope: String::new(),
            grant_id: grant.grant_ref().id.as_uuid().to_string(),
            generation: grant.grant_ref().generation.get(),
            executor: serde_json::to_string(&grant.executor().binding)?,
            now: u64::try_from(chrono::Utc::now().timestamp_millis())?,
            credit_records: 1,
            credit_bytes: 1000,
            snapshot_ceiling: 1000,
        };
        assert!(
            dsl::LiveRequestMachineMutator::apply(&mut candidate, reservation.clone()).is_err()
        );
        assert_eq!(candidate.state(), &before);
        let control = LiveSourceKey::new(
            source.session_id().clone(),
            source.channel_id().clone(),
            LiveSourceIdentity::ApplicationRequest {
                request_id: LiveApplicationRequestId::from_uuid(uuid::Uuid::new_v4()),
            },
        )?;
        if let dsl::LiveRequestInput::Reserve { source, .. } = &mut reservation {
            *source = serde_json::to_string(&control)?;
        }
        dsl::LiveRequestMachineMutator::apply(&mut candidate, reservation)?;
        // An unrelated reserved source under the same grant remains admissible.
        owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn native_source_cancellation_of_reserved_work_has_no_input_or_run() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let source_before = owned
            .fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?;
        let LiveSourceEntryRecord::Reservation { record: original } = source_before.record()?
        else {
            return Err("reservation".into());
        };
        let intent = cancel_intent(owned.source.clone());
        owned.machine.cancel_live_request(intent).await?;
        let after = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let LiveSourceEntryRecord::Reservation { record } = owned
            .fixture
            .ops()?
            .lookup_live_source(&owned.source)
            .await?
            .ok_or("source")?
            .record()?
        else {
            return Err("reservation".into());
        };
        assert_eq!(record.frozen_digest()?, original.frozen_digest()?);
        assert_eq!(record.reserved_frontier(), original.reserved_frontier());
        assert!(matches!(
            record.disposition(),
            LiveSourceDisposition::CancelledWithoutRun { .. }
        ));
        assert!(
            owned
                .machine
                .commit_live_input_admission(owned.source.clone(), &owned.grant)
                .await
                .is_err()
        );
        let state = crate::generated::live_request_state::decode(&after.payload.request_snapshot)?;
        assert!(state.request_inputs.is_empty());
        assert!(state.run_requests.is_empty());
        assert!(state.claim_ids.is_empty());
        assert_eq!(after.reference.event_count, before.reference.event_count);
        assert!(
            after
                .payload
                .used
                .checked_add(after.payload.reserved)?
                .fits_within(before.payload.used.checked_add(before.payload.reserved)?)
        );
        assert_eq!(
            after.payload.reserved,
            crate::live_ledger::authority::store::cancellation::reserved_charge(&state)?
                .checked_add(
                    crate::live_ledger::write::transcript_commit::reserved_charge(
                        &crate::generated::live_transcript_state::decode(
                            &after.payload.transcript_snapshot
                        )?,
                    )?
                )?
        );
        assert_eq!(
            after.payload.transcript_snapshot,
            before.payload.transcript_snapshot
        );
        assert!(state.request_completion_obligations.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn native_source_cancellation_delivers_to_actual_admitted_input() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (admitted, completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let intent = cancel_intent(owned.source.clone());
        assert_eq!(
            owned.machine.cancel_live_request(intent.clone()).await?,
            intent
        );
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            completion.ok_or("completion")?.wait(),
        )
        .await??;
        assert!(matches!(
            outcome,
            crate::CompletionOutcome::RuntimeTerminated { .. }
        ));
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        let request = state
            .source_requests
            .get(&serde_json::to_string(&owned.source)?)
            .ok_or("request")?;
        assert_eq!(
            state.request_phases.get(request),
            Some(&dsl::LiveRequestPhase::Terminal)
        );
        assert_eq!(
            state.request_inputs.get(request),
            Some(&admitted.record().input_id().to_string())
        );
        assert_eq!(state.request_credit_spent_records.get(request), Some(&1));
        assert!(state.run_requests.is_empty());
        assert_eq!(
            head.payload.reserved,
            crate::live_ledger::authority::store::cancellation::reserved_charge(&state)?
                .checked_add(
                    crate::live_ledger::write::transcript_commit::reserved_charge(
                        &crate::generated::live_transcript_state::decode(
                            &head.payload.transcript_snapshot
                        )?,
                    )?
                )?
        );
        assert!(state.request_completion_obligations.is_empty());
        owned.machine.cancel_live_request(intent).await?;
        let repeated = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        assert_eq!(repeated.reference.event_count, head.reference.event_count);
        assert_eq!(repeated.payload.used, head.payload.used);
    }
    Ok(())
}

#[tokio::test]
async fn native_source_cancellation_race_reloads_admission_instead_of_losing_input() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let intent = cancel_intent(owned.source.clone());
        let prepared = source_owner(&owned)
            .prepare_source_cancellation(intent.clone())
            .await?;
        let (_, completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        assert!(prepared.commit(owned.fixture.store.as_ref()).await.is_err());
        owned.machine.cancel_live_request(intent).await?;
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            completion.ok_or("completion")?.wait(),
        )
        .await??;
        assert!(matches!(
            outcome,
            crate::CompletionOutcome::RuntimeTerminated { .. }
        ));
    }
    Ok(())
}

#[tokio::test]
async fn native_source_cancellation_commit_blocks_stage_before_delivery() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (admitted, _) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let committed = source_owner(&owned)
            .cancel_source(cancel_intent(owned.source.clone()))
            .await?;
        assert_eq!(
            committed.target.as_ref().map(|target| &target.input_id),
            Some(admitted.record().input_id())
        );
        assert!(
            owned
                .machine
                .prepare_next_batch_for_live_scope_authority_test(
                    owned.fixture.session.id(),
                    admitted.record().input_id()
                )
                .await
                .is_err()
        );
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert!(state.run_requests.is_empty());
        assert!(state.claim_ids.is_empty());
    }

    Ok(())
}

#[tokio::test]
async fn native_source_cancellation_full_capacity_covers_escaped_keys_and_all_reasons() -> TestResult
{
    for backend in backends() {
        for reason in [
            LiveRequestCancellationReason::OperatorRequested,
            LiveRequestCancellationReason::GrantRevoked,
            LiveRequestCancellationReason::SessionArchived,
            LiveRequestCancellationReason::ExplicitSupersession,
        ] {
            let owned = OwnedFixture::new(backend).await?;
            let row = reserved_source(&owned.fixture, &"\\\"\n\u{0001}".repeat(2000)).await?;
            let mut image = serde_json::to_value(row.record()?)?;
            image["record"]["grant"] = serde_json::to_value(owned.grant.record().grant_ref())?;
            let row = LiveSourceRow::encode(&serde_json::from_value(image)?)?;
            super::super::seed_live_reservation_for_test(
                owned.fixture.store.as_ref(),
                &owned.grant,
                row.clone(),
                current_fence(),
            )
            .await?;
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            let quota = before.payload.used.checked_add(before.payload.reserved)?;
            let mut prepared = source_owner(&owned)
                .prepare_source_cancellation(LiveRequestCancelIntent {
                    source: row.source().clone(),
                    reason,
                })
                .await?;
            prepared.commit.quota = quota;
            prepared.commit(owned.fixture.store.as_ref()).await?;
            let after = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            assert!(
                after
                    .payload
                    .used
                    .checked_add(after.payload.reserved)?
                    .fits_within(quota)
            );
            assert_eq!(after.payload.used.records, before.payload.used.records);
            assert_eq!(after.reference.event_count, before.reference.event_count);
            assert!(after.payload.reserved.encoded_bytes < before.payload.reserved.encoded_bytes);
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_source_cancellation_creates_no_grant_or_execution_authority() -> TestResult {
    for backend in backends() {
        let fixture = Fixture::new(backend).await?;
        let machine = MeerkatMachine::persistent(
            Arc::clone(&fixture.store),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        machine
            .prepare_bindings(fixture.session.id().clone())
            .await?;
        let source = LiveSourceKey::new(
            fixture.session.id().clone(),
            meerkat_core::live_execution::LiveChannelId::new("voice"),
            LiveSourceIdentity::ApplicationRequest {
                request_id: LiveApplicationRequestId::from_uuid(uuid::Uuid::new_v4()),
            },
        )?;
        let intent = cancel_intent(source);
        assert_eq!(machine.cancel_live_request(intent.clone()).await?, intent);
        let head = fixture
            .ops()?
            .load_live_head(fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert!(state.grant_id.is_empty());
        assert_eq!(state.grant_generation, 0);
        assert!(!state.ingress_open);
        assert!(state.grant_revoked);
        assert!(state.request_ids.is_empty());
        assert!(state.run_requests.is_empty());
        assert!(state.claim_ids.is_empty());
        assert_eq!(state.source_cancellations.len(), 1);
    }
    Ok(())
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn native_source_cancellation_retains_intent_across_full_reopen_before_delivery() -> TestResult
{
    cold_cancellation_recovery(false).await
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn native_source_cancellation_runtime_loop_recovers_after_full_reopen() -> TestResult {
    cold_cancellation_recovery(true).await
}

#[cfg(feature = "sqlite-store")]
async fn cold_cancellation_recovery(attach_runtime_loop: bool) -> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let owned = OwnedFixture::new(backend).await?;
        let (admitted, completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        drop(completion);
        let input_id = admitted.record().input_id().clone();
        drop(admitted);
        let intent = cancel_intent(owned.source.clone());
        let committed = source_owner(&owned).cancel_source(intent.clone()).await?;
        assert_eq!(
            committed.target.as_ref().map(|target| &target.input_id),
            Some(&input_id)
        );
        drop(committed);
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
            _directory,
            path,
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
        let machine = Arc::new(MeerkatMachine::persistent(
            Arc::clone(&store),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        machine.prepare_bindings(session.id().clone()).await?;
        if attach_runtime_loop {
            machine
                .ensure_session_with_executor(
                    session.id().clone(),
                    Box::new(LegacyScopeExecutor::default()),
                )
                .await?;
            let recovered = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let row = store
                        .load_input_state(&LogicalRuntimeId::for_session(session.id()), &input_id)
                        .await?
                        .ok_or("cold input disappeared")?;
                    let outcome = crate::input_state::input_terminal_completion_outcome(
                        std::slice::from_ref(&row),
                        &input_id,
                    )?;
                    let head = store
                        .live_ledger_ops()
                        .ok_or("Live ops")?
                        .load_live_head(session.id())
                        .await?
                        .ok_or("head")?;
                    let state = crate::generated::live_request_state::decode(
                        &head.payload.request_snapshot,
                    )?;
                    let request = state
                        .source_requests
                        .get(&serde_json::to_string(&intent.source)?)
                        .ok_or("request")?;
                    if outcome.is_some()
                        && state.request_phases.get(request)
                            == Some(&dsl::LiveRequestPhase::Terminal)
                    {
                        return TestResult::Ok(());
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await;
            machine.unregister_session(session.id()).await?;
            recovered??;
        } else {
            assert_eq!(
                machine
                    .reconcile_live_request_cancellations(session.id())
                    .await?,
                vec![intent.clone()]
            );
        }
        let row = store
            .load_input_state(&LogicalRuntimeId::for_session(session.id()), &input_id)
            .await?
            .ok_or("input")?;
        let outcome = crate::input_state::input_terminal_completion_outcome(
            std::slice::from_ref(&row),
            &input_id,
        )?
        .ok_or("outcome")?;
        assert!(matches!(
            outcome,
            crate::CompletionOutcome::RuntimeTerminated { ref reason, .. }
                if reason == "Live request cancellation: OperatorRequested"
        ));
        assert_eq!(row.seed.attempt_count, 0);
        assert!(row.seed.last_run_id.is_none());
        let head = store
            .live_ledger_ops()
            .ok_or("Live ops")?
            .load_live_head(session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        let request = state
            .source_requests
            .get(&serde_json::to_string(&intent.source)?)
            .ok_or("request")?;
        assert_eq!(
            state.request_phases.get(request),
            Some(&dsl::LiveRequestPhase::Terminal)
        );
        assert!(state.run_requests.is_empty());
        assert!(
            crate::live_ledger::authority::store::LiveRequestStoreOwner::new(
                Arc::clone(&store),
                session.id().clone()
            )
            .pending_source_cancellations()
            .await?
            .is_empty()
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_source_cancellation_late_delivery_cannot_target_next_ordinary_run() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (_, completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let intent = cancel_intent(owned.source.clone());
        owned.machine.cancel_live_request(intent.clone()).await?;
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            completion.ok_or("completion")?.wait(),
        )
        .await??;
        let ordinary = crate::input::PromptInput::new("next ordinary run", None);
        let input_id = ordinary.header.id.clone();
        owned
            .machine
            .accept_input_with_completion(
                owned.fixture.session.id(),
                crate::input::Input::Prompt(ordinary),
            )
            .await?;
        let (run, authority) = owned
            .machine
            .prepare_next_batch_for_live_scope_authority_test(owned.fixture.session.id(), &input_id)
            .await?;
        assert!(matches!(
            authority,
            meerkat_core::execution_scope::RunExecutionAuthority::SessionPolicy
        ));
        let runtime = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let before = owned
            .fixture
            .store
            .load_input_states_by_ids_with_versions(&runtime, std::slice::from_ref(&input_id))
            .await?;
        assert!(
            source_owner(&owned)
                .cancel_source(intent.clone())
                .await?
                .target
                .is_none()
        );
        owned.machine.cancel_live_request(intent).await?;
        let after = owned
            .fixture
            .store
            .load_input_states_by_ids_with_versions(&runtime, std::slice::from_ref(&input_id))
            .await?;
        let before = before[0].as_ref().ok_or("ordinary input")?;
        let after = after[0].as_ref().ok_or("ordinary input")?;
        assert_eq!(before.exact_row_digest(), after.exact_row_digest());
        assert_eq!(after.state().seed.last_run_id.as_ref(), Some(&run));
        assert_eq!(
            after.state().seed.phase,
            crate::input_state::InputLifecycleState::Staged
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_source_cancellation_runtime_loop_recovers_before_queue_staging() -> TestResult {
    runtime_loop_cancellation(false).await
}

#[tokio::test]
async fn native_source_cancellation_runtime_loop_orders_cancel_after_observation() -> TestResult {
    runtime_loop_cancellation(true).await
}

async fn runtime_loop_cancellation(cancel_after_observation: bool) -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (admitted, completion) = owned
            .machine
            .commit_live_input_admission(owned.source.clone(), &owned.grant)
            .await?;
        let pause = if cancel_after_observation {
            Some(
                owned
                    .machine
                    .arm_runtime_loop_before_queue_authority_test_hook(
                        owned.fixture.session.id().clone(),
                    ),
            )
        } else {
            source_owner(&owned)
                .cancel_source(cancel_intent(owned.source.clone()))
                .await?;
            None
        };
        owned
            .machine
            .ensure_session_with_executor(
                owned.fixture.session.id().clone(),
                Box::new(LegacyScopeExecutor::default()),
            )
            .await?;
        if let Some((entered, release)) = pause {
            tokio::time::timeout(std::time::Duration::from_secs(5), entered).await??;
            source_owner(&owned)
                .cancel_source(cancel_intent(owned.source.clone()))
                .await?;
            release
                .send(())
                .map_err(|()| "queue pause receiver closed")?;
        }
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            completion.ok_or("completion")?.wait(),
        )
        .await;
        let durability_ready = owned
            .machine
            .is_durability_ready(owned.fixture.session.id())
            .await;
        let row = owned
            .fixture
            .store
            .load_input_state(
                &LogicalRuntimeId::for_session(owned.fixture.session.id()),
                admitted.record().input_id(),
            )
            .await;
        owned
            .machine
            .unregister_session(owned.fixture.session.id())
            .await?;
        let outcome = outcome??;
        assert!(durability_ready);
        let row = row?.ok_or("cancelled input")?;
        assert_eq!(row.seed.attempt_count, 0);
        assert!(row.seed.last_run_id.is_none());
        assert!(
            matches!(outcome, crate::CompletionOutcome::RuntimeTerminated { ref reason, .. }
            if reason == "Live request cancellation: OperatorRequested"),
            "{outcome:?}"
        );
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert!(state.run_requests.is_empty());
        assert!(state.claim_ids.is_empty());
    }
    Ok(())
}
