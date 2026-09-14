use super::restore_tests::scope_owner;
use super::*;
use crate::completion::CompletionOutcome;
use crate::input_state::{
    InputLifecycleState, InputStatePersistenceRecord, InputTerminalCompletion,
    InputTerminalCompletionBatchKey, InputTerminalCompletionFinalizationVerdict,
    InputTerminalCompletionPhase, InputTerminalOutcome, input_terminal_completion_outcome,
    interaction_terminal_payload_digest,
};
use crate::live_ledger::authority::store::LiveRequestAuthorityError;
use crate::live_ledger::authority::store::request_completion::LiveRequestCompletionProgress;
use crate::live_ledger::completion::{LiveCompletionEvent, LiveRequestCompletionFact};
use crate::live_ledger::record::LiveLedgerRecord;
use crate::store::live_history::LiveHistoryReadRequest;
use meerkat_core::execution_scope::ScopedRunAuthority;
use meerkat_core::{InputId, OperationId};

async fn cancel_admitted_runless(
    owned: &OwnedFixture,
) -> TestResult<(InputId, OperationId, CompletionOutcome)> {
    let (admitted, completion) = owned
        .machine
        .commit_live_input_admission(owned.source.clone(), &owned.grant)
        .await?;
    let session_id = owned.fixture.session.id();
    let input_id = admitted.record().input_id();
    assert!(
        owned
            .machine
            .cancel_input_if_present(session_id, input_id, "cancel before Live run")
            .await?
    );
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        completion.ok_or("input completion")?.wait(),
    )
    .await??;
    assert!(
        matches!(outcome,
            CompletionOutcome::RuntimeTerminated { ref reason, .. } if reason == "cancel before Live run"
        ),
        "{outcome:?}"
    );
    let runtime = LogicalRuntimeId::for_session(session_id);
    let row = owned
        .fixture
        .store
        .load_input_state(&runtime, input_id)
        .await?
        .ok_or("input")?;
    assert_eq!(row.seed.last_run_id, None);
    assert_eq!(row.seed.last_boundary_sequence, None);
    let retained = input_terminal_completion_outcome(std::slice::from_ref(&row), input_id)?
        .ok_or("retained runless outcome")?;
    assert_eq!(
        serde_json::to_value(retained)?,
        serde_json::to_value(&outcome)?
    );
    let request_id = match owned
        .fixture
        .ops()?
        .lookup_live_source(&owned.source)
        .await?
        .ok_or("source")?
        .record()?
    {
        crate::live_source::LiveSourceEntryRecord::Reservation { record } => {
            record.request_id().clone()
        }
        _ => return Err("request source changed kind".into()),
    };
    Ok((input_id.clone(), request_id, outcome))
}

#[tokio::test]
async fn native_request_completion_uses_real_runless_input_terminal_owner() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let (input_id, request_id, _) = cancel_admitted_runless(&owned).await?;
        let session_id = owned.fixture.session.id();
        let head = owned
            .fixture
            .ops()?
            .load_live_head(session_id)
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert_eq!(
            state.request_phases.get(&request_id.to_string()),
            Some(&dsl::LiveRequestPhase::Terminal),
            "ordinary finalization must drive Live closure without an explicit observer"
        );
        let owner = scope_owner(&owned);
        let LiveRequestCompletionProgress::Committed(sequence) =
            owner.reconcile_request_completion(&request_id).await?
        else {
            return Err("runless terminal did not close its exact Live request".into());
        };
        let head = owned
            .fixture
            .ops()?
            .load_live_head(session_id)
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert!(state.request_runs.is_empty());
        assert!(state.run_scopes.is_empty());
        assert!(state.claim_ids.is_empty());
        assert_eq!(
            state.request_inputs.get(&request_id.to_string()),
            Some(&input_id.to_string())
        );
        assert_eq!(
            state
                .request_credit_spent_records
                .get(&request_id.to_string()),
            Some(&1)
        );
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
        let history = owned
            .fixture
            .ops()?
            .read_live_history(&LiveHistoryReadRequest::new(
                head.reference.clone(),
                None,
                0,
                64,
            )?)
            .await?;
        let references = history
            .records()
            .iter()
            .filter_map(|record| match record {
                LiveLedgerRecord::Completion(record) => match &record.event {
                    LiveCompletionEvent::RequestOutcome {
                        request_id: observed,
                        outcome,
                    } if observed == &request_id => Some(outcome),
                    _ => None,
                },
                _ => None,
            })
            .collect::<Vec<_>>();
        let [
            LiveRequestCompletionFact::OrdinaryRunlessTerminal {
                input_id: observed,
                receipt_digest,
            },
        ] = references.as_slice()
        else {
            return Err(format!("expected one runless reference: {references:?}").into());
        };
        assert_eq!(observed, &input_id);
        assert_eq!(
            state
                .request_ordinary_completion_digests
                .get(&request_id.to_string())
                .map(String::as_str),
            Some(receipt_digest.as_str())
        );
        assert_eq!(
            owner.reconcile_request_completion(&request_id).await?,
            LiveRequestCompletionProgress::Committed(sequence)
        );
        assert!(
            owned
                .machine
                .prepare_next_batch_for_live_scope_authority_test(session_id, &input_id)
                .await
                .is_err(),
            "cancelled runless input must not stage"
        );
        let runtime = LogicalRuntimeId::for_session(session_id);
        let mut row = owned
            .fixture
            .store
            .load_input_state(&runtime, &input_id)
            .await?
            .ok_or("row")?;
        row.seed.last_run_id = Some(meerkat_core::RunId::new());
        owned
            .fixture
            .store
            .persist_input_state(
                &runtime,
                &InputStatePersistenceRecord::from_machine_snapshot(row)?,
            )
            .await?;
        assert!(matches!(
            owner.reconcile_request_completion(&request_id).await,
            Err(LiveRequestAuthorityError::InvalidOrdinaryCompletion(_))
        ));
        assert_eq!(
            owned.fixture.ops()?.load_live_head(session_id).await?,
            Some(head)
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_request_completion_recovers_real_runless_cancellation_after_append_failure()
-> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        for shared_batch in [false, true] {
            let owned = OwnedFixture::new(backend).await?;
            let conn = meerkat_sqlite::open(
                &owned.fixture.path,
                meerkat_sqlite::ConnectionProfile::PRIMARY,
            )?;
            conn.execute_batch(
                "CREATE TRIGGER reject_live_completion BEFORE INSERT ON runtime_live_events
             BEGIN SELECT RAISE(ABORT, 'injected Live completion append failure'); END;",
            )?;
            // Admission updates the head/source, not the event log. Only Live followup is faulted.
            let (completion_inputs, request_id, outcome) = if shared_batch {
                let ids = terminate_shared_runless(&owned).await?;
                let source = owned
                    .fixture
                    .ops()?
                    .lookup_live_source(&owned.source)
                    .await?
                    .ok_or("source")?;
                let crate::live_source::LiveSourceEntryRecord::Reservation { record } =
                    source.record()?
                else {
                    return Err("request source lost its reservation".into());
                };
                let rows = owned
                    .fixture
                    .store
                    .load_input_states_by_ids(
                        &LogicalRuntimeId::for_session(owned.fixture.session.id()),
                        &ids,
                    )
                    .await?
                    .into_iter()
                    .collect::<Option<Vec<_>>>()
                    .ok_or("batch")?;
                let outcome =
                    input_terminal_completion_outcome(&rows, &ids[0])?.ok_or("outcome")?;
                (ids, record.request_id().clone(), outcome)
            } else {
                let (id, request, outcome) = cancel_admitted_runless(&owned).await?;
                (vec![id], request, outcome)
            };
            let input_id = &completion_inputs[0];
            let session_id = owned.fixture.session.id();
            let before = owned
                .fixture
                .ops()?
                .load_live_head(session_id)
                .await?
                .ok_or("head")?;
            let state =
                crate::generated::live_request_state::decode(&before.payload.request_snapshot)?;
            assert_eq!(
                state.request_phases.get(&request_id.to_string()),
                Some(&dsl::LiveRequestPhase::Admitted)
            );
            let owner = scope_owner(&owned);
            assert!(matches!(
                owner.reconcile_request_completion(&request_id).await,
                Err(LiveRequestAuthorityError::Store(RuntimeStoreError::WriteFailed(ref reason)))
                    if reason.contains("injected Live completion append failure")
            ));
            let runtime = LogicalRuntimeId::for_session(session_id);
            let mut row = owned
                .fixture
                .store
                .load_input_state(
                    &runtime,
                    completion_inputs.last().ok_or("completion input")?,
                )
                .await?
                .ok_or("row")?;
            let owner_id = &row
                .state
                .terminal_completion
                .as_ref()
                .ok_or("completion")?
                .owner_input_id;
            let owner_row = owned
                .fixture
                .store
                .load_input_state(&runtime, owner_id)
                .await?
                .ok_or("owner row")?;
            let completion = owner_row
                .state
                .terminal_completion
                .as_ref()
                .ok_or("completion owner")?;
            assert!(completion.candidate.is_none());
            assert!(completion.outcome.is_some());
            assert!(
                owned
                    .fixture
                    .store
                    .load_input_states_with_versions(&runtime)
                    .await?
                    .into_parts()
                    .0
                    .is_empty()
            );
            let stale = owner.prepare_request_completion(&request_id).await?;
            row.state.updated_at += chrono::Duration::seconds(1);
            owned
                .fixture
                .store
                .persist_input_state(
                    &runtime,
                    &InputStatePersistenceRecord::from_machine_snapshot(row)?,
                )
                .await?;
            conn.execute_batch("DROP TRIGGER reject_live_completion;")?;
            assert!(matches!(
                owner.commit_request_completion(stale).await,
                Err(LiveRequestAuthorityError::Store(
                    RuntimeStoreError::InputRowVersionConflict { .. }
                ))
            ));
            assert_eq!(
                owned.fixture.ops()?.load_live_head(session_id).await?,
                Some(before.clone())
            );
            drop(conn);
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
                Backend::HeadCanonical => {
                    crate::store::SqliteRuntimeStore::new_head_canonical(&path)?
                }
                Backend::Memory => return Err("durable backend required".into()),
            });
            let ops = store.live_ledger_ops().ok_or("Live ops")?;
            assert_eq!(ops.load_live_head(session.id()).await?, Some(before));
            let owner = crate::live_ledger::authority::store::LiveRequestStoreOwner::new(
                Arc::clone(&store),
                session.id().clone(),
            );
            let reconciliations = owner.reconcile_input_completions(None).await?;
            assert_eq!(reconciliations.len(), 1);
            let reconciliation = reconciliations.into_iter().next().ok_or("reconciliation")?;
            assert_eq!(reconciliation.request_id, request_id);
            let progress = reconciliation.result?;
            assert!(matches!(
                progress,
                LiveRequestCompletionProgress::Committed(_)
            ));
            let committed = ops.load_live_head(session.id()).await?;
            assert!(owner.reconcile_input_completions(None).await?.is_empty());
            assert_eq!(
                owner.reconcile_request_completion(&request_id).await?,
                progress
            );
            assert_eq!(ops.load_live_head(session.id()).await?, committed);
            let rows = store
                .load_input_states_by_ids(&runtime, &completion_inputs)
                .await?
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or("retained batch")?;
            let retained = input_terminal_completion_outcome(&rows, input_id)?
                .ok_or("retained full outcome")?;
            assert_eq!(
                serde_json::to_value(retained)?,
                serde_json::to_value(outcome)?
            );
        }
    }
    Ok(())
}

async fn terminate_shared_runless(owned: &OwnedFixture) -> TestResult<Vec<InputId>> {
    let (admitted, live_waiter) = owned
        .machine
        .commit_live_input_admission(owned.source.clone(), &owned.grant)
        .await?;
    let ordinary = crate::input::PromptInput::new("ordinary queued sibling", None);
    let inputs = vec![
        admitted.record().input_id().clone(),
        ordinary.header.id.clone(),
    ];
    let (_, ordinary_waiter) = owned
        .machine
        .accept_input_with_completion(
            owned.fixture.session.id(),
            crate::input::Input::Prompt(ordinary),
        )
        .await?;
    let waiters = [
        live_waiter.ok_or("Live waiter")?,
        ordinary_waiter.ok_or("ordinary waiter")?,
    ];
    owned
        .machine
        .unregister_session(owned.fixture.session.id())
        .await?;
    let runtime = LogicalRuntimeId::for_session(owned.fixture.session.id());
    let rows = owned
        .fixture
        .store
        .load_input_states_by_ids(&runtime, &inputs)
        .await?
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or("retained batch")?;
    for (input_id, waiter) in inputs.iter().zip(waiters) {
        let outcome =
            tokio::time::timeout(std::time::Duration::from_secs(5), waiter.wait()).await??;
        assert!(matches!(
            outcome,
            CompletionOutcome::RuntimeTerminated { .. }
        ));
        let retained =
            input_terminal_completion_outcome(&rows, input_id)?.ok_or("retained outcome")?;
        assert_eq!(
            serde_json::to_value(retained)?,
            serde_json::to_value(outcome)?
        );
    }
    Ok(inputs)
}

#[tokio::test]
async fn native_request_completion_closes_shared_runless_batch_on_real_teardown() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let inputs = terminate_shared_runless(&owned).await?;
        let head = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
        assert_eq!(state.admitted_requests.len(), 1);
        assert!(state.request_runs.is_empty());
        for request in &state.admitted_requests {
            assert_eq!(
                state.request_phases.get(request),
                Some(&dsl::LiveRequestPhase::Terminal)
            );
        }
        let runtime = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let observations = owned
            .fixture
            .store
            .load_input_states_by_ids_with_versions(&runtime, &inputs)
            .await?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or("exact batch")?;
        let change = prepared(owned.fixture.session.id(), Some(&head), vec![])?
            .with_completion_batch_fence(&observations, &inputs[0])?;
        assert_eq!(change.input_read_fences().len(), 2);
        let mut sibling = observations[1].state().clone();
        sibling.state.updated_at += chrono::Duration::seconds(1);
        owned
            .fixture
            .store
            .persist_input_state(
                &runtime,
                &InputStatePersistenceRecord::from_machine_snapshot(sibling)?,
            )
            .await?;
        assert!(
            matches!(
                owned.fixture.ops()?.commit_live_ledger(change, current_fence()).await,
                Err(RuntimeStoreError::InputRowVersionConflict { input_id, .. }) if input_id == inputs[1].to_string()
            ),
            "mechanical ledger commits must compare the non-Live sibling too"
        );
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            Some(head)
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_request_completion_batch_reads_are_exact_ordered_and_bounded() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let inputs = terminate_shared_runless(&owned).await?;
        let runtime = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let missing = InputId::new();
        let order = [inputs[1].clone(), missing, inputs[0].clone()];
        let observations = owned
            .fixture
            .store
            .load_input_states_by_ids_with_versions(&runtime, &order)
            .await?;
        assert_eq!(observations.len(), order.len());
        assert!(observations[1].is_none());
        for index in [0, 2] {
            let row = observations[index].as_ref().ok_or("row")?;
            assert_eq!(row.state().state.input_id, order[index]);
            let before = row.exact_row_digest();
            let mut changed = row.state().clone();
            changed.state.updated_at += chrono::Duration::seconds(1);
            owned
                .fixture
                .store
                .persist_input_state(
                    &runtime,
                    &InputStatePersistenceRecord::from_machine_snapshot(changed)?,
                )
                .await?;
            let refreshed = owned
                .fixture
                .store
                .load_input_states_by_ids_with_versions(
                    &runtime,
                    std::slice::from_ref(&order[index]),
                )
                .await?;
            assert_ne!(
                refreshed[0].as_ref().ok_or("refreshed")?.exact_row_digest(),
                before
            );
        }
        assert!(
            owned
                .fixture
                .store
                .load_input_states_by_ids_with_versions(
                    &runtime,
                    &[inputs[0].clone(), inputs[0].clone()]
                )
                .await
                .is_err()
        );
        let oversized: Vec<_> = (0..=crate::store::MAX_INPUT_STATE_BATCH_CAS)
            .map(|_| InputId::new())
            .collect();
        assert!(
            owned
                .fixture
                .store
                .load_input_states_by_ids_with_versions(&runtime, &oversized)
                .await
                .is_err()
        );
        assert!(
            owned
                .fixture
                .store
                .load_input_states_by_ids_with_versions(&runtime, &[])
                .await?
                .is_empty()
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_request_completion_cannot_reclassify_a_bound_run_as_runless() -> TestResult {
    let owned = OwnedFixture::new(Backend::Memory).await?;
    let scope = owned.staged_scope().await?;
    let head = owned
        .fixture
        .ops()?
        .load_live_head(owned.fixture.session.id())
        .await?
        .ok_or("head")?;
    let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
    let owner = dsl::LiveRequestMachineAuthority::recover_from_state(state.clone())?;
    for input in [
        dsl::LiveRequestInput::CompleteRunless {
            request_id: scope.record().request_id.to_string(),
            input_id: scope.record().input_id.to_string(),
            ordinary_completion_digest: "a".repeat(64),
            completion_records: 1,
            completion_bytes: 1,
            completion_sequence: 1,
            completion_digest: "b".repeat(64),
        },
        dsl::LiveRequestInput::ObserveRunlessCompletion {
            request_id: scope.record().request_id.to_string(),
            input_id: scope.record().input_id.to_string(),
            ordinary_completion_digest: "a".repeat(64),
        },
    ] {
        let mut candidate = owner.prepare_authority();
        assert!(dsl::LiveRequestMachineMutator::apply(&mut candidate, input).is_err());
        assert_eq!(candidate.state(), &state);
    }
    Ok(())
}

#[tokio::test]
async fn native_request_completion_rediscovers_retired_receipt_after_failed_append_and_full_reopen()
-> TestResult {
    for backend in [Backend::WholeBlob, Backend::HeadCanonical] {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let request_id = scope.record().request_id.clone();
        let owner = scope_owner(&owned);
        seed_finalized_ordinary(&owned, &scope, completed_result(&owned)).await?;
        let pending = owner.prepare_request_completion(&request_id).await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let runtime_id = LogicalRuntimeId::for_session(owned.fixture.session.id());
        let mut row = owned
            .fixture
            .store
            .load_input_state(&runtime_id, &scope.record().input_id)
            .await?
            .ok_or("row")?;
        row.state.updated_at += chrono::Duration::seconds(1);
        owned
            .fixture
            .store
            .persist_input_state(
                &runtime_id,
                &InputStatePersistenceRecord::from_machine_snapshot(row)?,
            )
            .await?;
        assert!(matches!(
            owner.commit_request_completion(pending).await,
            Err(LiveRequestAuthorityError::Store(
                RuntimeStoreError::InputRowVersionConflict { .. }
            ))
        ));
        assert!(
            owned
                .fixture
                .store
                .load_input_states_with_versions(&runtime_id)
                .await?
                .into_parts()
                .0
                .is_empty(),
            "ordinary nonterminal recovery cannot discover the retired finalized row"
        );
        drop(owner);
        drop(scope);
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
        let ops = store.live_ledger_ops().ok_or("Live ops")?;
        assert_eq!(ops.load_live_head(session.id()).await?, before);
        let owner = crate::live_ledger::authority::store::LiveRequestStoreOwner::new(
            Arc::clone(&store),
            session.id().clone(),
        );
        let reconciliations = owner.reconcile_input_completions(None).await?;
        assert_eq!(reconciliations.len(), 1);
        let reconciliation = reconciliations.into_iter().next().ok_or("reconciliation")?;
        assert_eq!(reconciliation.request_id, request_id);
        let progress = reconciliation.result?;
        assert!(matches!(
            progress,
            LiveRequestCompletionProgress::Committed(_)
        ));
        let committed = ops.load_live_head(session.id()).await?;
        assert!(owner.reconcile_input_completions(None).await?.is_empty());
        assert_eq!(
            owner.reconcile_request_completion(&request_id).await?,
            progress
        );
        assert_eq!(ops.load_live_head(session.id()).await?, committed);
    }
    Ok(())
}

fn completed_result(owned: &OwnedFixture) -> CompletionOutcome {
    CompletionOutcome::Completed(Box::new(meerkat_core::RunResult {
        text: "\0".repeat(crate::live_ledger::completion::LIVE_RESULT_MAX_BYTES + 1),
        session_id: owned.fixture.session.id().clone(),
        usage: meerkat_core::Usage::default(),
        turns: 1,
        tool_calls: 0,
        terminal_cause_kind: Some(meerkat_core::TurnTerminalCauseKind::BudgetExhausted),
        structured_output: Some(serde_json::json!({"exact": ["retained", 17]})),
        extraction_error: None,
        schema_warnings: None,
        skill_diagnostics: None,
    }))
}

// Explicit ordinary-owner storage fixture, not execution/finalization evidence.
// Admission and staging are real; only the independent owner's terminal row is seeded.
pub(super) async fn seed_finalized_ordinary(
    owned: &OwnedFixture,
    scope: &ScopedRunAuthority,
    outcome: CompletionOutcome,
) -> TestResult<String> {
    let runtime = LogicalRuntimeId::for_session(owned.fixture.session.id());
    let mut row = owned
        .fixture
        .store
        .load_input_state(&runtime, &scope.record().input_id)
        .await?
        .ok_or("input")?;
    let finalization = InputTerminalCompletionFinalizationVerdict::Succeeded;
    let receipt_digest = interaction_terminal_payload_digest(&(&outcome, finalization))?;
    let input_ids = vec![row.state.input_id.clone()];
    row.seed.phase = InputLifecycleState::Consumed;
    row.seed.terminal_outcome = Some(InputTerminalOutcome::Consumed);
    row.seed.recovery_lane = None;
    row.state.persisted_input = None;
    row.state.terminal_completion = Some(InputTerminalCompletion {
        input_id: row.state.input_id.clone(),
        batch_ordinal: 0,
        batch_key: InputTerminalCompletionBatchKey::Run {
            run_id: scope.record().run_id.clone(),
        },
        owner_input_id: row.state.input_id.clone(),
        candidate_digest: interaction_terminal_payload_digest(&outcome)?,
        completion_input_ids_digest: interaction_terminal_payload_digest(&input_ids)?,
        requires_session_checkpoint: true,
        candidate: None,
        completion_input_ids: Some(input_ids),
        outcome: Some(outcome),
        phase: InputTerminalCompletionPhase::Finalized {
            receipt_digest: receipt_digest.clone(),
            finalization,
        },
    });
    let record = InputStatePersistenceRecord::from_machine_snapshot(row)?;
    owned
        .fixture
        .store
        .persist_input_state(&runtime, &record)
        .await?;
    Ok(receipt_digest)
}

#[tokio::test]
async fn native_request_completion_consumes_exact_retired_ordinary_receipt_once() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let owner = scope_owner(&owned);
        assert_eq!(
            owner
                .reconcile_request_completion(&scope.record().request_id)
                .await?,
            LiveRequestCompletionProgress::OrdinaryPending
        );
        let outcome = completed_result(&owned);
        let expected = serde_json::to_value(&outcome)?;
        let receipt_digest = seed_finalized_ordinary(&owned, &scope, outcome).await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let pending = owner
            .prepare_request_completion(&scope.record().request_id)
            .await?;
        let duplicate = owner
            .prepare_request_completion(&scope.record().request_id)
            .await?;
        let progress = owner.commit_request_completion(pending).await?;
        let LiveRequestCompletionProgress::Committed(sequence) = progress else {
            return Err("completion not committed".into());
        };
        assert_eq!(sequence.get(), before.reference.event_count + 1);
        assert_eq!(owner.commit_request_completion(duplicate).await?, progress);
        let committed = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("head")?;
        let state =
            crate::generated::live_request_state::decode(&committed.payload.request_snapshot)?;
        assert_eq!(
            state
                .request_phases
                .get(&scope.record().request_id.to_string()),
            Some(&dsl::LiveRequestPhase::Terminal)
        );
        assert_eq!(
            state
                .request_ordinary_completion_digests
                .get(&scope.record().request_id.to_string()),
            Some(&receipt_digest)
        );
        assert_eq!(
            committed.payload.reserved,
            crate::live_ledger::authority::store::cancellation::reserved_charge(&state)?
                .checked_add(
                    crate::live_ledger::write::transcript_commit::reserved_charge(
                        &crate::generated::live_transcript_state::decode(
                            &committed.payload.transcript_snapshot
                        )?,
                    )?
                )?
        );
        assert!(state.request_completion_obligations.is_empty());
        assert_eq!(
            committed.payload.transcript_snapshot,
            before.payload.transcript_snapshot
        );
        assert_eq!(
            owner
                .reconcile_request_completion(&scope.record().request_id)
                .await?,
            progress
        );
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            Some(committed)
        );
        let row = owned
            .fixture
            .store
            .load_input_state(
                &LogicalRuntimeId::for_session(owned.fixture.session.id()),
                &scope.record().input_id,
            )
            .await?
            .ok_or("retained row")?;
        assert!(row.state.persisted_input.is_none());
        assert!(
            row.state
                .terminal_completion
                .as_ref()
                .ok_or("completion")?
                .candidate
                .is_none()
        );
        assert_eq!(
            serde_json::to_value(
                crate::input_state::input_terminal_completion_outcome(
                    std::slice::from_ref(&row),
                    &scope.record().input_id,
                )?
                .ok_or("outcome")?
            )?,
            expected,
            "large text, typed budget cause, and structured output must remain exact"
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_request_completion_rechecks_exact_finalized_row_before_publication() -> TestResult {
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let owner = scope_owner(&owned);
        seed_finalized_ordinary(&owned, &scope, completed_result(&owned)).await?;
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
            LiveRequestCompletionProgress::Committed(_)
        ));
    }
    Ok(())
}

#[tokio::test]
async fn native_callback_completion_rejects_claim_with_foreign_call_content() -> TestResult {
    use super::claim_tests::{read_only_observation, tool_target_for_call};
    use crate::live_ledger::completion::{LiveCompletionText, LivePhysicalEffectOutcome};
    use meerkat_core::session::CallbackBatchIdentity;
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let identity: CallbackBatchIdentity = serde_json::from_value(serde_json::json!({
            "session_id": owned.fixture.session.id(),
            "run_id": scope.record().run_id,
            "execution_scope": scope.scope_id(),
            "execution_boundary": meerkat_core::ops::OperationId::new(),
            "batch_digest": vec![7; 32],
        }))?;
        let permit = owned
            .machine
            .claim_live_effect(
                scope.clone(),
                identity
                    .scoped_tool_effect_id("callback")?
                    .ok_or("callback effect")?,
                tool_target_for_call(
                    "another-call",
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
                LiveCompletionText::new("observed")?,
            )
            .await?;
        seed_finalized_ordinary(
            &owned,
            &scope,
            CompletionOutcome::CallbackPending {
                tool_use_id: "callback".into(),
                tool_name: "callback_view".into(),
                args: serde_json::json!({}),
                callback_identity: Some(identity),
            },
        )
        .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        assert!(
            scope_owner(&owned)
                .reconcile_request_completion(&scope.record().request_id)
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

#[tokio::test]
async fn native_callback_completion_requires_exact_observed_effect_lineage() -> TestResult {
    use super::claim_tests::{read_only_observation, tool_target_for_call};
    use crate::live_ledger::completion::{LiveCompletionText, LivePhysicalEffectOutcome};
    use meerkat_core::session::CallbackBatchIdentity;
    for backend in backends() {
        let owned = OwnedFixture::new(backend).await?;
        let scope = owned.staged_scope().await?;
        let identity_json = serde_json::json!({
            "session_id": owned.fixture.session.id(),
            "run_id": scope.record().run_id,
            "execution_scope": scope.scope_id(),
            "execution_boundary": meerkat_core::ops::OperationId::new(),
            "batch_digest": vec![7; 32],
        });
        let identity: CallbackBatchIdentity = serde_json::from_value(identity_json.clone())?;
        let effect_id = identity
            .scoped_tool_effect_id("callback")?
            .ok_or("callback effect")?;
        let permit = owned
            .machine
            .claim_live_effect(
                scope.clone(),
                effect_id,
                tool_target_for_call(
                    "callback",
                    "allowed_tool",
                    meerkat_core::ToolMutationClass::ReadOnly,
                ),
                read_only_observation()?,
            )
            .await?;
        let outcome = |identity| CompletionOutcome::CallbackPending {
            tool_use_id: "callback".into(),
            tool_name: "callback_view".into(),
            args: serde_json::json!({}),
            callback_identity: Some(identity),
        };
        seed_finalized_ordinary(&owned, &scope, outcome(identity.clone())).await?;
        let owner = scope_owner(&owned);
        assert!(
            owner
                .reconcile_request_completion(&scope.record().request_id)
                .await
                .is_err(),
            "a Claimed effect is not an observed callback"
        );
        let before_feedback = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("prior head")?;
        owned
            .machine
            .settle_live_effect(
                permit.into_claim(),
                LivePhysicalEffectOutcome::Unknown,
                LiveCompletionText::new("callback pending")?,
            )
            .await?;
        let before = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        let suspended_progress = owner
            .reconcile_request_completion(&scope.record().request_id)
            .await?;
        assert!(matches!(
            suspended_progress,
            LiveRequestCompletionProgress::CallbackSuspended(_)
        ));
        let suspended = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?
            .ok_or("suspended head")?;
        let state =
            crate::generated::live_request_state::decode(&suspended.payload.request_snapshot)?;
        assert_eq!(
            state
                .request_phases
                .get(&scope.record().request_id.to_string()),
            Some(&dsl::LiveRequestPhase::Suspended),
            "an observed callback must durably suspend its Live request",
        );
        assert_eq!(
            suspended.reference.event_count,
            before_feedback.reference.event_count + 2,
            "physical feedback must trigger one durable suspension record",
        );
        assert_eq!(
            Some(&suspended),
            before.as_ref(),
            "explicit reconciliation must replay the suspension"
        );
        let before = Some(suspended);
        for field in ["execution_boundary", "execution_scope", "session_id"] {
            let mut changed = identity_json.clone();
            changed[field] = serde_json::to_value(meerkat_core::ops::OperationId::new())?;
            seed_finalized_ordinary(&owned, &scope, outcome(serde_json::from_value(changed)?))
                .await?;
            assert!(
                owner
                    .reconcile_request_completion(&scope.record().request_id)
                    .await
                    .is_err(),
                "{field}"
            );
        }
        seed_finalized_ordinary(
            &owned,
            &scope,
            CompletionOutcome::CallbackBatchPending {
                pending_tool_calls: vec![
                    meerkat_core::error::PendingCallbackToolCall {
                        tool_use_id: "callback".into(),
                        tool_name: "callback_view".into(),
                        args: serde_json::json!({}),
                    };
                    2
                ],
                callback_identity: Some(identity.clone()),
            },
        )
        .await?;
        assert!(
            owner
                .reconcile_request_completion(&scope.record().request_id)
                .await
                .is_err()
        );
        let mut historical = identity_json;
        historical
            .as_object_mut()
            .ok_or("identity object")?
            .remove("execution_boundary");
        seed_finalized_ordinary(&owned, &scope, outcome(serde_json::from_value(historical)?))
            .await?;
        assert_eq!(
            owner
                .reconcile_request_completion(&scope.record().request_id)
                .await?,
            LiveRequestCompletionProgress::CallbackUnattributed,
        );
        assert_eq!(
            owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?,
            before
        );
        seed_finalized_ordinary(&owned, &scope, outcome(identity)).await?;
        owner
            .commit(
                dsl::LiveRequestInput::Revoke {
                    grant_id: scope.record().grant.id.as_uuid().to_string(),
                    generation: scope.record().grant.generation.get(),
                },
                current_fence(),
            )
            .await?;
        assert_eq!(
            owner
                .reconcile_request_completion(&scope.record().request_id)
                .await?,
            suspended_progress,
            "revocation does not erase an already-observed physical callback",
        );
        if matches!(backend, Backend::Memory) {
            continue;
        }
        let request_id = scope.record().request_id.clone();
        let retained = owned
            .fixture
            .ops()?
            .load_live_head(owned.fixture.session.id())
            .await?;
        drop(owner);
        drop(scope);
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
        let cold = crate::live_ledger::authority::store::LiveRequestStoreOwner::new(
            Arc::clone(&store),
            session.id().clone(),
        );
        assert_eq!(
            cold.reconcile_request_completion(&request_id).await?,
            suspended_progress,
        );
        assert_eq!(
            store
                .live_ledger_ops()
                .ok_or("Live ops")?
                .load_live_head(session.id())
                .await?,
            retained
        );
    }
    Ok(())
}

#[tokio::test]
async fn native_request_completion_does_not_close_callback_or_unclassified_outcomes() -> TestResult
{
    for backend in backends() {
        for (outcome, expected) in [
            (
                CompletionOutcome::CallbackPending {
                    tool_use_id: "call".into(),
                    tool_name: "tool".into(),
                    args: serde_json::json!({}),
                    callback_identity: None,
                },
                LiveRequestCompletionProgress::CallbackUnattributed,
            ),
            (
                CompletionOutcome::CallbackBatchPending {
                    pending_tool_calls: vec![meerkat_core::error::PendingCallbackToolCall {
                        tool_use_id: "call".into(),
                        tool_name: "tool".into(),
                        args: serde_json::json!({}),
                    }],
                    callback_identity: None,
                },
                LiveRequestCompletionProgress::CallbackUnattributed,
            ),
            (
                CompletionOutcome::CompletedWithoutResult,
                LiveRequestCompletionProgress::OrdinaryUnclassified,
            ),
        ] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            seed_finalized_ordinary(&owned, &scope, outcome).await?;
            let before = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?;
            assert_eq!(
                scope_owner(&owned)
                    .reconcile_request_completion(&scope.record().request_id)
                    .await?,
                expected
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
    }
    Ok(())
}
