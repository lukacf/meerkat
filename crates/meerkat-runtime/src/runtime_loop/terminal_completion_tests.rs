#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::input::{InputOrigin, PeerConvention, PeerInput, PromptInput};
use crate::input_state::{InputTerminalCompletionPhase, InteractionTerminalCandidate};
use crate::meerkat_machine::driver as owner;
use crate::meerkat_machine::dsl as mm;
use crate::store::{RuntimeStore, SqliteRuntimeStore};
use meerkat_core::lifecycle::CoreApplyFailureCause;
use meerkat_core::types::SessionId;
use std::sync::Arc;

const FAILURE: &str =
    "Internal error: terminal peer-response apply intent requires exactly one SystemNotice append";

struct NoExecution;

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutor for NoExecution {
    async fn cancel_after_boundary(&mut self, _: String) -> Result<(), CoreExecutorError> {
        panic!("receipt recovery must not cancel a run")
    }

    async fn stop_runtime_executor(&mut self, _: String) -> Result<(), CoreExecutorError> {
        panic!("receipt recovery must not stop a run")
    }

    async fn apply(
        &mut self,
        _: RunId,
        _: meerkat_core::lifecycle::run_primitive::RunPrimitive,
    ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, CoreExecutorError> {
        panic!("receipt recovery must not execute a turn")
    }

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        _: Arc<Vec<u8>>,
    ) -> Result<(), CoreExecutorError> {
        panic!("an abandoned pre-executor input needs no checkpoint")
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<SqliteRuntimeStore>,
    driver: owner::SharedDriver,
    runtime_id: crate::identifiers::LogicalRuntimeId,
    session_id: SessionId,
    inputs: Vec<InputId>,
    run_id: RunId,
    carrier: Arc<RuntimeLoopTeardownSlot>,
}

impl Fixture {
    async fn failed_batch() -> Self {
        Self::failed_batch_with_cause(CoreApplyFailureCause::executor_internal(FAILURE)).await
    }

    async fn failed_batch_with_cause(failure: CoreApplyFailureCause) -> Self {
        Self::failed_batch_with_placement(failure, false).await
    }

    async fn failed_batch_with_placement(
        failure: CoreApplyFailureCause,
        registered_placement: bool,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteRuntimeStore::new(dir.path().join("runtime.sqlite")).unwrap());
        let session_id = SessionId::new();
        let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
        let mut persistent = PersistentRuntimeDriver::new(
            runtime_id.clone(),
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        if registered_placement {
            persistent
                .inner_mut()
                .install_registered_authority_for_test(
                    mm::SessionId::from_domain(&session_id),
                    Some(&runtime_id),
                    Some(1),
                    Some(mm::Generation::from(1)),
                    Some(mm::RuntimeEpochId::from(
                        "abandoned-failure-epoch".to_owned(),
                    )),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                )
                .expect("directed terminal fixture requires registered placement");
        }
        let driver = Arc::new(crate::tokio::sync::Mutex::new(
            owner::DriverEntry::Persistent(persistent),
        ));
        let (inputs, run_id, carrier) = Self::fail_another_batch(&driver, failure).await;
        Self {
            _dir: dir,
            store,
            driver,
            runtime_id,
            session_id,
            inputs,
            run_id,
            carrier,
        }
    }

    async fn fail_another_batch(
        driver: &owner::SharedDriver,
        failure: CoreApplyFailureCause,
    ) -> (Vec<InputId>, RunId, Arc<RuntimeLoopTeardownSlot>) {
        let mut inputs = Vec::new();
        for body in ["first independent input", "second independent input"] {
            let mut header = PromptInput::new("", None).header;
            header.source = InputOrigin::Peer {
                peer_id: "completion-fixture".into(),
                display_identity: None,
                runtime_id: None,
            };
            let input = Input::Peer(PeerInput {
                header,
                directed_interaction_id: None,
                objective_id: None,
                system_prompts: Vec::new(),
                injected_context: Vec::new(),
                sender_taint: None,
                convention: Some(PeerConvention::Message),
                content: body.into(),
                payload: None,
                handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
            });
            inputs.push(input.id().clone());
            assert!(
                driver
                    .lock()
                    .await
                    .as_driver_mut()
                    .accept_input(input)
                    .await
                    .unwrap()
                    .is_accepted()
            );
        }
        let mut last_run = None;
        let carrier = RuntimeLoopTeardownSlot::pending();
        for attempt in 0..3 {
            let batch = owner::machine_authorize_runtime_loop_batch(&*driver.lock().await).unwrap();
            assert_eq!(batch.input_ids().len(), 2);
            let run_id = RunId::new();
            assert!(matches!(
                owner::prepare_runtime_loop_batch_start(driver, run_id.clone(), batch)
                    .await
                    .unwrap(),
                owner::RuntimeLoopBatchStart::Started
            ));
            if attempt == 2 {
                let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
                let (_, outcome) = realize_runtime_loop_terminal_owned(
                    gate.lock_owned().await,
                    carrier.clone(),
                    driver.clone(),
                    run_id.clone(),
                    inputs.clone(),
                    false,
                    OwnedRuntimeLoopTerminalization::Failed {
                        failure: failure.clone(),
                        contributor_disposition: owner::FailedRunContributorDisposition::Replayed,
                    },
                )
                .await
                .unwrap();
                assert!(matches!(
                    outcome,
                    OwnedRuntimeLoopTerminalizationOutcome::Persisted
                ));
            } else {
                owner::fail_runtime_loop_run(
                    driver,
                    run_id.clone(),
                    failure.clone(),
                    owner::FailedRunContributorDisposition::Replayed,
                )
                .await
                .unwrap();
            }
            last_run = Some(run_id);
        }
        (inputs, last_run.unwrap(), carrier)
    }

    fn raw_row(&self, input_id: &InputId) -> Vec<u8> {
        rusqlite::Connection::open(self.store.path())
            .unwrap()
            .query_row(
                "SELECT state_json FROM runtime_input_states WHERE runtime_id=?1 AND input_id=?2",
                rusqlite::params![self.runtime_id.to_string(), input_id.to_string()],
                |row| row.get(0),
            )
            .unwrap()
    }

    async fn pending(&self) {
        for input_id in &self.inputs {
            let row = self
                .store
                .load_input_state(&self.runtime_id, input_id)
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                row.seed.terminal_outcome,
                Some(crate::input_state::InputTerminalOutcome::Abandoned { .. })
            ));
            let completion = row.state.terminal_completion.unwrap();
            assert_eq!(completion.batch_key.run_id(), Some(&self.run_id));
            assert!(!completion.requires_session_checkpoint);
            assert!(matches!(
                completion.phase,
                InputTerminalCompletionPhase::Pending
            ));
            assert!(row.state.interaction_terminal_outbox.is_none());
        }
    }

    async fn finalized(&self) -> Vec<Vec<u8>> {
        let mut bytes = Vec::new();
        for input_id in &self.inputs {
            let row = self
                .store
                .load_input_state(&self.runtime_id, input_id)
                .await
                .unwrap()
                .unwrap();
            let completion = row.state.terminal_completion.as_ref().unwrap();
            assert!(matches!(
                completion.phase,
                InputTerminalCompletionPhase::Finalized { .. }
            ));
            if completion.owner_input_id == *input_id {
                let outcome = completion.outcome.as_ref().unwrap();
                let crate::completion::CompletionOutcome::AbandonedWithError { error, .. } =
                    outcome
                else {
                    panic!("expected the original failed attempt, got {outcome:?}");
                };
                assert_eq!(
                    error,
                    &meerkat_core::TurnErrorMetadata::runtime_apply_failure(FAILURE)
                );
            } else {
                assert!(completion.outcome.is_none());
            }
            bytes.push(serde_json::to_vec(&row).unwrap());
        }
        bytes
    }

    async fn later_attached(&self) -> mm::MeerkatMachineState {
        let mut entry = self.driver.lock().await;
        let newer = RunId::new();
        owner::machine_begin_run(&mut entry, newer.clone()).unwrap();
        let shared = entry.shared_dsl_authority();
        let mut machine = shared.lock().unwrap();
        for input in [
            mm::MeerkatMachineInput::RunCancelled {
                run_id: mm::RunId::from_domain(&newer),
            },
            mm::MeerkatMachineInput::Commit {
                input_id: mm::InputId::from_domain(&InputId::new()),
                run_id: mm::RunId::from_domain(&newer),
            },
            mm::MeerkatMachineInput::PrepareBindings {
                session_id: mm::SessionId::from_domain(&self.session_id),
                agent_runtime_id: mm::AgentRuntimeId::from("abandoned-completion-fixture"),
                fence_token: mm::FenceToken::from(1),
                generation: Some(mm::Generation::from(0)),
                runtime_epoch_id: None,
            },
        ] {
            mm::MeerkatMachineMutator::apply(&mut *machine, input).unwrap();
        }
        let state = machine.state().clone();
        assert_eq!(state.lifecycle_phase, mm::MeerkatPhase::Attached);
        assert_eq!(
            state.runtime_completion_result_run_id,
            Some(mm::RunId::from_domain(&newer))
        );
        drop(machine);
        if let owner::DriverEntry::Persistent(driver) = &mut *entry {
            driver
                .inner_mut()
                .sync_control_projection_from_dsl_authority();
        }
        state
    }
}

fn assert_live_run_unchanged(before: &mm::MeerkatMachineState, after: &mm::MeerkatMachineState) {
    assert_eq!(before.lifecycle_phase, after.lifecycle_phase);
    assert_eq!(before.current_run_id, after.current_run_id);
    assert_eq!(before.turn_terminal_run_id, after.turn_terminal_run_id);
    assert_eq!(
        before.runtime_completion_result_run_id,
        after.runtime_completion_result_run_id
    );
    assert_eq!(
        before.runtime_completion_result_resolved,
        after.runtime_completion_result_resolved
    );
    assert_eq!(before.terminal_outcome, after.terminal_outcome);
    assert_eq!(before.terminal_cause_kind, after.terminal_cause_kind);
}

#[tokio::test]
async fn abandoned_completion_recovers_from_attached_without_replacing_later_run() {
    let fixture = Fixture::failed_batch().await;
    fixture.pending().await;
    let before = fixture.later_attached().await;
    let error = owner::machine_recover_runtime_completion_result_correlation(
        &*fixture.driver.lock().await,
        &fixture.run_id,
        crate::input_state::RuntimeCompletionTerminalRecovery::MachineFailure {
            outcome: meerkat_core::TurnTerminalOutcome::Failed,
            cause: meerkat_core::TurnTerminalCauseKind::RuntimeApplyFailure,
        },
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("guard rejected transition from Attached")
    );
    drain_recovered_input_terminal_completions(&fixture.driver, None, &mut NoExecution)
        .await
        .unwrap();
    let finalized = fixture.finalized().await;
    let shared = fixture.driver.lock().await.shared_dsl_authority();
    assert_live_run_unchanged(&before, shared.lock().unwrap().state());
    drain_recovered_input_terminal_completions(&fixture.driver, None, &mut NoExecution)
        .await
        .unwrap();
    assert_eq!(finalized, fixture.finalized().await);
}

#[tokio::test]
async fn failed_completion_payload_rejection_retains_retry_carrier_and_pending_rows() {
    let fixture = Fixture::failed_batch().await;
    let completions = Arc::new(crate::tokio::sync::Mutex::new(
        crate::completion::CompletionRegistry::new(),
    ));
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    let _ = resolve_machine_terminal_completion_waiters_under_authority(
        &fixture.driver,
        Some(&completions),
        gate.lock_owned().await,
        &fixture.carrier,
        &fixture.inputs,
        &fixture.run_id,
        format!("runtime turn-state preparation failed: {FAILURE}"),
    )
    .await;
    fixture.pending().await;
    assert!(
        fixture.carrier.pending_nondirected_run_terminal().is_some(),
        "a rejected delivery must retain its exact durable retry owner"
    );
}

#[tokio::test]
async fn failed_completion_exact_payload_finalizes_the_real_producer_batch() {
    let fixture = Fixture::failed_batch().await;
    let completions = Arc::new(crate::tokio::sync::Mutex::new(
        crate::completion::CompletionRegistry::new(),
    ));
    let mut handles = Vec::new();
    for id in &fixture.inputs {
        handles.push(completions.lock().await.register(id.clone()));
    }
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    let _ = resolve_machine_terminal_completion_waiters_under_authority(
        &fixture.driver,
        Some(&completions),
        gate.lock_owned().await,
        &fixture.carrier,
        &fixture.inputs,
        &fixture.run_id,
        FAILURE.to_owned(),
    )
    .await;
    fixture.finalized().await;
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    for handle in handles {
        let outcome = handle.wait_authorized().await;
        assert!(matches!(
            outcome,
            crate::completion::CompletionOutcome::AbandonedWithError { .. }
        ));
    }
}

#[tokio::test]
async fn current_run_abandoned_receipt_finalizes_without_rewriting_live_correlation() {
    let fixture = Fixture::failed_batch().await;
    fixture.pending().await;
    let shared = fixture.driver.lock().await.shared_dsl_authority();
    let before = shared.lock().unwrap().state().clone();
    assert_eq!(
        before.runtime_completion_result_run_id,
        Some(mm::RunId::from_domain(&fixture.run_id))
    );
    assert!(!before.runtime_completion_result_resolved);
    {
        let entry = fixture.driver.lock().await;
        let witness = entry
            .input_terminal_completion_authorization_witness(&fixture.inputs)
            .unwrap();
        assert_eq!(
            owner::machine_abandoned_completion_error_for_batch(&entry, &witness).unwrap(),
            Some(meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                FAILURE
            ))
        );
    }
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    resolve_machine_terminal_completion_waiters(
        &fixture.driver,
        None,
        gate.lock_owned().await,
        &fixture.carrier,
        &fixture.inputs,
        &fixture.run_id,
        FAILURE.to_owned(),
    )
    .await
    .unwrap();
    let finalized = fixture.finalized().await;
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    let mut resolved = before;
    resolved.runtime_completion_result_resolved = true;
    assert_live_run_unchanged(&resolved, shared.lock().unwrap().state());
    // Finalizing this run's receipt settles its result slot without changing
    // the correlation or terminal lineage. A repeated drain is idempotent.
    drain_recovered_input_terminal_completions(&fixture.driver, None, &mut NoExecution)
        .await
        .unwrap();
    assert_eq!(finalized, fixture.finalized().await);
    assert_live_run_unchanged(&resolved, shared.lock().unwrap().state());
}

#[tokio::test]
async fn stage_refusal_terminal_remains_recoverable_after_a_prior_abandoned_failure() {
    let fixture = Fixture::failed_batch_with_placement(
        CoreApplyFailureCause::executor_internal(FAILURE),
        true,
    )
    .await;
    fixture.pending().await;
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    resolve_machine_terminal_completion_waiters(
        &fixture.driver,
        None,
        gate.lock_owned().await,
        &fixture.carrier,
        &fixture.inputs,
        &fixture.run_id,
        FAILURE.to_owned(),
    )
    .await
    .unwrap();
    let prior_receipts = fixture.finalized().await;
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());

    let mut header = PromptInput::new("", None).header;
    let input_id = header.id.clone();
    let interaction_id = meerkat_core::interaction::InteractionId(input_id.0);
    header.source = InputOrigin::Peer {
        peer_id: "stage-refusal-after-abandoned-failure".into(),
        display_identity: None,
        runtime_id: Some(crate::identifiers::LogicalRuntimeId::new(
            "stage-refusal-after-abandoned-failure",
        )),
    };
    header.idempotency_key = Some(crate::identifiers::IdempotencyKey::new(
        interaction_id.to_string(),
    ));
    header.correlation_id = Some(crate::identifiers::CorrelationId::from_uuid(input_id.0));
    let input = Input::Peer(PeerInput {
        header,
        directed_interaction_id: Some(interaction_id),
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        convention: Some(PeerConvention::Message),
        content: "directed input after an abandoned failure".into(),
        payload: None,
        handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
    });
    assert!(
        fixture
            .driver
            .lock()
            .await
            .as_driver_mut()
            .accept_input(input)
            .await
            .unwrap()
            .is_accepted()
    );

    let mut refused_run = None;
    for _ in 0..8 {
        let run_id = RunId::new();
        let outcome = owner::prepare_runtime_loop_batch_start(
            &fixture.driver,
            run_id.clone(),
            owner::test_authorized_runtime_loop_batch(vec![input_id.clone(), InputId::new()]),
        )
        .await
        .expect("staging refusal must remain a typed non-fatal outcome");
        let owner::RuntimeLoopBatchStart::StageRefused {
            abandoned_input_ids,
            ..
        } = outcome
        else {
            panic!("the unstageable batch must never execute a turn");
        };
        if abandoned_input_ids == [input_id.clone()] {
            refused_run = Some(run_id);
            break;
        }
    }
    let run_id = refused_run.expect("the generated attempt cap must terminalize the input");
    assert_ne!(run_id, fixture.run_id);
    let pending = fixture
        .store
        .load_input_state(&fixture.runtime_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        pending.state.terminal_completion.unwrap().phase,
        InputTerminalCompletionPhase::Pending
    ));
    assert!(pending.state.interaction_terminal_outbox.is_some());

    let mut entry = fixture.driver.lock().await;
    let mut batches = entry
        .interaction_terminal_recovery_batches()
        .await
        .unwrap_or_else(|error| {
            let shared = entry.shared_dsl_authority();
            let machine = shared.lock().unwrap();
            panic!(
                "a finalized abandoned failure must not block the next refused batch: {error}; \
                 requested_run={run_id} result_run={:?} result_resolved={} turn_terminal_run={:?}",
                machine.state().runtime_completion_result_run_id,
                machine.state().runtime_completion_result_resolved,
                machine.state().turn_terminal_run_id,
            )
        });
    assert_eq!(batches.len(), 1);
    let batch = batches.remove(0);
    assert_eq!(batch.input_ids, vec![input_id.clone()]);
    assert_eq!(batch.batch_key.run_id(), Some(&run_id));
    owner::machine_recover_runtime_completion_result_correlation(
        &entry,
        &run_id,
        batch.terminal_recovery,
    )
    .expect("recover the next stage-refusal terminal correlation");
    assert!(
        owner::machine_recover_runtime_completion_result_correlation(
            &entry,
            &run_id,
            crate::input_state::RuntimeCompletionTerminalRecovery::Cancelled,
        )
        .is_err(),
        "recovery must still reject a contradictory terminal overwrite"
    );
    let authority = owner::machine_resolve_runtime_completion_result(
        &entry,
        Some(&run_id),
        batch.terminal_observation,
        mm::RuntimeCompletionFinalizationObservation::Succeeded,
    )
    .unwrap();
    let witness = entry
        .input_terminal_completion_authorization_witness(&batch.input_ids)
        .unwrap();
    let bundle = crate::completion::authorize_runtime_terminal_bundle(
        &batch.interaction_ids,
        batch.terminal.as_ref(),
        authority,
        witness,
        batch.completion_error_metadata,
        None,
    )
    .unwrap();
    let events = bundle.interaction_events().to_vec();
    assert!(matches!(
        events.as_slice(),
        [meerkat_core::event::AgentEvent::InteractionFailed { interaction_id, .. }]
            if interaction_id.0 == input_id.0
    ));
    entry
        .finalize_input_terminal_completion_batch(bundle.terminal_completion())
        .await
        .unwrap();
    entry
        .finalize_interaction_terminal_outboxes(
            &input_id,
            &events,
            mm::RuntimeCompletionFinalizationObservation::Succeeded,
        )
        .await
        .unwrap();
    let receipts = events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt::try_new(
                event,
                u64::try_from(index).unwrap() + 1,
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    entry
        .mark_interaction_terminal_outboxes_published(&input_id, &receipts)
        .await
        .unwrap();
    assert!(
        entry
            .interaction_terminal_recovery_batches()
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        entry
            .input_terminal_completion_recovery_batches()
            .await
            .unwrap()
            .is_empty()
    );
    drop(entry);

    let mut completions = crate::completion::CompletionRegistry::new();
    let handle = completions.register(input_id.clone());
    completions.resolve_authorized_runtime_terminal_bundle([input_id.clone()], bundle);
    let completion = handle.try_wait_with_terminal_outcome().await.unwrap();
    assert_eq!(
        completion.input_terminal_outcome(),
        Some(&crate::input_state::InputTerminalOutcome::Abandoned {
            reason: crate::input_state::InputAbandonReason::MaxAttemptsExhausted { attempts: 3 },
        })
    );
    let finalized = fixture
        .store
        .load_input_state(&fixture.runtime_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        finalized.state.terminal_completion.unwrap().phase,
        InputTerminalCompletionPhase::Finalized { .. }
    ));
    assert_eq!(prior_receipts, fixture.finalized().await);
}

#[tokio::test]
async fn failed_completion_without_waiters_still_finalizes_durable_receipts() {
    let fixture = Fixture::failed_batch().await;
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    let _ = resolve_machine_terminal_completion_waiters(
        &fixture.driver,
        None,
        gate.lock_owned().await,
        &fixture.carrier,
        &fixture.inputs,
        &fixture.run_id,
        FAILURE.to_owned(),
    )
    .await;
    fixture.finalized().await;
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
}

#[tokio::test]
async fn abandoned_completion_rejects_altered_candidate_without_touching_later_run() {
    let fixture = Fixture::failed_batch().await;
    let before = fixture.later_attached().await;
    let row = fixture
        .store
        .load_input_state(&fixture.runtime_id, &fixture.inputs[0])
        .await
        .unwrap()
        .unwrap();
    let owner_id = row.state.terminal_completion.unwrap().owner_input_id;
    let mut owner_row = fixture
        .store
        .load_input_state(&fixture.runtime_id, &owner_id)
        .await
        .unwrap()
        .unwrap();
    owner_row
        .state
        .terminal_completion
        .as_mut()
        .unwrap()
        .candidate = Some(InteractionTerminalCandidate::MachineTerminalFailure {
        error: meerkat_core::TurnErrorMetadata::runtime_apply_failure("altered candidate"),
    });
    if let owner::DriverEntry::Persistent(driver) = &mut *fixture.driver.lock().await {
        driver
            .inner_mut()
            .insert_input_state_for_test(owner_row.state.clone());
    }
    rusqlite::Connection::open(fixture.store.path())
        .unwrap()
        .execute(
            "UPDATE runtime_input_states SET state_json=?3 WHERE runtime_id=?1 AND input_id=?2",
            rusqlite::params![
                fixture.runtime_id.to_string(),
                owner_id.to_string(),
                serde_json::to_vec(&owner_row).unwrap()
            ],
        )
        .unwrap();
    assert!(
        drain_recovered_input_terminal_completions(&fixture.driver, None, &mut NoExecution)
            .await
            .is_err()
    );
    let shared = fixture.driver.lock().await.shared_dsl_authority();
    assert_live_run_unchanged(&before, shared.lock().unwrap().state());
    let retained: Vec<u8> = rusqlite::Connection::open(fixture.store.path())
        .unwrap()
        .query_row(
            "SELECT state_json FROM runtime_input_states WHERE runtime_id=?1 AND input_id=?2",
            rusqlite::params![fixture.runtime_id.to_string(), owner_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, serde_json::to_vec(&owner_row).unwrap());
    assert!(matches!(
        owner_row.state.terminal_completion.as_ref().unwrap().phase,
        InputTerminalCompletionPhase::Pending
    ));
}

#[tokio::test]
async fn failed_completion_pre_executor_causes_share_exact_durable_and_retry_detail() {
    for failure in [
        CoreApplyFailureCause::primitive_rejected(FAILURE),
        CoreApplyFailureCause::executor_internal(FAILURE),
    ] {
        let fixture = Fixture::failed_batch_with_cause(failure.clone()).await;
        let pending = fixture.carrier.pending_nondirected_run_terminal().unwrap();
        assert_eq!(
            pending.completion_error_metadata.unwrap().detail.as_deref(),
            Some(failure.message())
        );
        let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
        resolve_machine_terminal_completion_waiters(
            &fixture.driver,
            None,
            gate.lock_owned().await,
            &fixture.carrier,
            &fixture.inputs,
            &fixture.run_id,
            failure.message().to_owned(),
        )
        .await
        .unwrap();
        fixture.finalized().await;
    }
}

#[tokio::test]
async fn abandoned_completion_requires_whole_batch_and_exact_run() {
    let fixture = Fixture::failed_batch().await;
    let before = fixture.later_attached().await;
    let entry = fixture.driver.lock().await;
    assert!(
        entry
            .input_terminal_completion_authorization_witness(&fixture.inputs[..1])
            .is_err()
    );
    let witness = entry
        .input_terminal_completion_authorization_witness(&fixture.inputs)
        .unwrap();
    assert!(
        owner::machine_resolve_runtime_completion_result_for_batch(
            &entry,
            &witness,
            Some(&RunId::new()),
            mm::RuntimeCompletionTerminalObservation::MachineTerminal,
            mm::RuntimeCompletionFinalizationObservation::Succeeded,
        )
        .is_err()
    );
    let shared = entry.shared_dsl_authority();
    assert_live_run_unchanged(&before, shared.lock().unwrap().state());
}

#[tokio::test]
async fn abandoned_completion_generated_authority_rejects_wrong_facts() {
    let fixture = Fixture::failed_batch().await;
    let before = fixture.later_attached().await;
    let entry = fixture.driver.lock().await;
    let first = entry
        .as_driver()
        .stored_input_state(&fixture.inputs[0])
        .unwrap();
    let owner_id = first.state.terminal_completion.unwrap().owner_input_id;
    let completion = entry
        .as_driver()
        .stored_input_state(&owner_id)
        .unwrap()
        .state
        .terminal_completion
        .unwrap();
    let shared = entry.shared_dsl_authority();
    for invalid in 0..10 {
        let mut input = mm::MeerkatMachineInput::ResolveAbandonedCompletionResult {
            owner_input_id: owner_id.to_string(),
            run_id: mm::RunId::from_domain(&fixture.run_id),
            candidate_digest: completion.candidate_digest.clone(),
            completion_input_ids_digest: completion.completion_input_ids_digest.clone(),
            recipient_input_ids: fixture.inputs.iter().map(ToString::to_string).collect(),
            terminal_outcome: mm::TurnTerminalOutcome::Failed,
            terminal_cause_kind: mm::TurnTerminalCauseKind::RuntimeApplyFailure,
            requires_session_checkpoint: false,
            has_interaction_terminal_outbox: false,
            finalization: mm::RuntimeCompletionFinalizationObservation::Succeeded,
        };
        if let mm::MeerkatMachineInput::ResolveAbandonedCompletionResult {
            owner_input_id,
            run_id,
            candidate_digest,
            completion_input_ids_digest,
            recipient_input_ids,
            terminal_outcome,
            terminal_cause_kind,
            requires_session_checkpoint,
            has_interaction_terminal_outbox,
            finalization,
        } = &mut input
        {
            match invalid {
                0 => *owner_input_id = InputId::new().to_string(),
                1 => *run_id = mm::RunId::from_domain(&RunId::new()),
                2 => candidate_digest.clear(),
                3 => completion_input_ids_digest.clear(),
                4 => recipient_input_ids.clear(),
                5 => *terminal_outcome = mm::TurnTerminalOutcome::Cancelled,
                6 => *terminal_cause_kind = mm::TurnTerminalCauseKind::FatalFailure,
                7 => *requires_session_checkpoint = true,
                8 => *has_interaction_terminal_outbox = true,
                9 => *finalization = mm::RuntimeCompletionFinalizationObservation::Failed,
                _ => unreachable!(),
            }
        }
        let mut machine = shared.lock().unwrap();
        assert!(
            mm::MeerkatMachineMutator::apply(&mut *machine, input).is_err(),
            "accepted invalid fact {invalid}"
        );
        assert_live_run_unchanged(&before, machine.state());
    }
}

#[tokio::test]
async fn failed_completion_receipt_write_fault_retains_carrier_then_retries_atomically() {
    let fixture = Fixture::failed_batch().await;
    let shared = fixture.driver.lock().await.shared_dsl_authority();
    let live_before = shared.lock().unwrap().state().clone();
    assert_eq!(
        live_before.runtime_completion_result_run_id,
        Some(mm::RunId::from_domain(&fixture.run_id))
    );
    assert!(!live_before.runtime_completion_result_resolved);
    let before = fixture
        .inputs
        .iter()
        .map(|id| fixture.raw_row(id))
        .collect::<Vec<_>>();
    let completions = Arc::new(crate::tokio::sync::Mutex::new(
        crate::completion::CompletionRegistry::new(),
    ));
    let mut waiters = Vec::new();
    for input_id in &fixture.inputs {
        let handle = completions.lock().await.register(input_id.clone());
        waiters.push(tokio::spawn(async move { handle.wait_authorized().await }));
    }
    rusqlite::Connection::open(fixture.store.path())
        .unwrap()
        .execute_batch(&format!(
            "CREATE TRIGGER fail_terminal_receipt BEFORE UPDATE ON runtime_input_states
         WHEN NEW.runtime_id = '{}' AND NEW.input_id = '{}'
         BEGIN SELECT RAISE(ABORT, 'synthetic terminal receipt CAS fault'); END;",
            fixture.runtime_id, fixture.inputs[1],
        ))
        .unwrap();
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    let result = resolve_machine_terminal_completion_waiters(
        &fixture.driver,
        Some(&completions),
        gate.clone().lock_owned().await,
        &fixture.carrier,
        &fixture.inputs,
        &fixture.run_id,
        FAILURE.to_owned(),
    )
    .await;
    assert!(result.is_err());
    assert_live_run_unchanged(&live_before, shared.lock().unwrap().state());
    assert_eq!(
        before,
        fixture
            .inputs
            .iter()
            .map(|id| fixture.raw_row(id))
            .collect::<Vec<_>>()
    );
    fixture.pending().await;
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_some());
    assert!(waiters.iter().all(|waiter| !waiter.is_finished()));
    rusqlite::Connection::open(fixture.store.path())
        .unwrap()
        .execute_batch("DROP TRIGGER fail_terminal_receipt;")
        .unwrap();
    resolve_machine_terminal_completion_waiters(
        &fixture.driver,
        Some(&completions),
        gate.lock_owned().await,
        &fixture.carrier,
        &fixture.inputs,
        &fixture.run_id,
        FAILURE.to_owned(),
    )
    .await
    .unwrap();
    fixture.finalized().await;
    let mut resolved = live_before;
    resolved.runtime_completion_result_resolved = true;
    assert_live_run_unchanged(&resolved, shared.lock().unwrap().state());
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    for waiter in waiters {
        let result = crate::tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        let crate::completion::CompletionOutcome::AbandonedWithError { error, .. } = result else {
            panic!("unexpected waiter outcome {result:?}");
        };
        assert_eq!(
            error,
            meerkat_core::TurnErrorMetadata::runtime_apply_failure(FAILURE)
        );
    }
}

#[tokio::test]
async fn committed_receipt_rejection_requires_reload_and_preserves_retry_evidence() {
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    let session_id = SessionId::new();
    let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
    let health = owner::ready_durability_health_for_test();
    let shared = crate::driver::ephemeral::new_ingress_dsl_authority();
    let mut persistent = PersistentRuntimeDriver::new_with_control_and_durability_health(
        runtime_id.clone(),
        store.clone(),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
        Arc::new(std::sync::RwLock::new(
            crate::driver::ephemeral::RuntimeControlProjection::default(),
        )),
        shared.clone(),
        health.clone(),
    );
    persistent
        .inner_mut()
        .install_registered_authority_for_test(
            mm::SessionId::from_domain(&session_id),
            Some(&runtime_id),
            Some(1),
            Some(mm::Generation::from(1)),
            Some(mm::RuntimeEpochId::from(
                "receipt-rejection-epoch".to_owned(),
            )),
            crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
        )
        .unwrap();
    let driver = Arc::new(crate::tokio::sync::Mutex::new(
        owner::DriverEntry::Persistent(persistent),
    ));
    let (inputs, run_id, carrier) =
        Fixture::fail_another_batch(&driver, CoreApplyFailureCause::executor_internal(FAILURE))
            .await;
    assert!(health.require_ready().is_ok());
    assert!(
        !shared
            .lock()
            .unwrap()
            .state()
            .runtime_completion_result_resolved
    );

    let entered = Arc::new(crate::tokio::sync::Notify::new());
    let release = Arc::new(crate::tokio::sync::Notify::new());
    store.block_next_input_state_batch_cas_after_commit(entered.clone(), release.clone());
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    let task = tokio::spawn({
        let driver = driver.clone();
        let carrier = carrier.clone();
        let inputs = inputs.clone();
        let run_id = run_id.clone();
        let gate = gate.clone();
        async move {
            resolve_machine_terminal_completion_waiters(
                &driver,
                None,
                gate.lock_owned().await,
                &carrier,
                &inputs,
                &run_id,
                FAILURE.to_owned(),
            )
            .await
        }
    });
    crate::tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
        .await
        .expect("receipt CAS must commit before rejection is injected");

    let mut committed = Vec::new();
    for input_id in &inputs {
        let row = store
            .load_input_state(&runtime_id, input_id)
            .await
            .unwrap()
            .unwrap();
        let receipt = row.state.terminal_completion.as_ref().unwrap();
        assert!(matches!(
            receipt.phase,
            InputTerminalCompletionPhase::Finalized { .. }
        ));
        assert_eq!(receipt.batch_key.run_id(), Some(&run_id));
        if receipt.owner_input_id == *input_id {
            let Some(crate::completion::CompletionOutcome::AbandonedWithError { error, .. }) =
                receipt.outcome.as_ref()
            else {
                panic!("committed owner lost its exact terminal failure");
            };
            assert_eq!(
                *error,
                meerkat_core::TurnErrorMetadata::runtime_apply_failure(FAILURE)
            );
        }
        committed.push(serde_json::to_vec(&row).unwrap());
    }

    // Fault injection: shared generated authority loses its registration after
    // the real store commits, while every recipient remains available. This
    // forces post-CAS receipt realization to reject without editing DSL state.
    {
        let mut machine = shared.lock().unwrap();
        let state = machine.state().clone();
        let session_id = state.session_id.clone().unwrap();
        let transitions = [
            mm::MeerkatMachineInput::BeginUnregisterSession {
                session_id: session_id.clone(),
                agent_runtime_id: state.active_runtime_id.clone(),
                fence_token: state.active_fence_token,
                generation: state.active_runtime_generation,
                runtime_epoch_id: state.active_runtime_epoch_id.clone(),
            },
            mm::MeerkatMachineInput::RuntimeLoopStoppedForUnregister {
                session_id: session_id.clone(),
                forced_abort: false,
            },
            mm::MeerkatMachineInput::CommsDrainExitedForUnregister {
                session_id: session_id.clone(),
                forced_abort: false,
            },
            mm::MeerkatMachineInput::CompletionWaitersResolvedForUnregister {
                session_id: session_id.clone(),
            },
            mm::MeerkatMachineInput::UnregisterSession {
                session_id,
                agent_runtime_id: state.active_runtime_id,
                fence_token: state.active_fence_token,
                generation: state.active_runtime_generation,
                runtime_epoch_id: state.active_runtime_epoch_id,
            },
        ];
        for input in transitions {
            mm::MeerkatMachineMutator::apply(&mut *machine, input).unwrap();
        }
        assert!(machine.state().session_id.is_none());
        for input_id in &inputs {
            assert!(
                machine
                    .state()
                    .input_phases
                    .contains_key(&input_id.to_string())
            );
        }
    }
    release.notify_one();
    let error = crate::tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("terminal completion receipt realization")
    );
    let required = health.require_ready().unwrap_err();
    assert_eq!(
        required.operation(),
        "terminal completion receipt realization"
    );
    assert!(required.reason().contains("guard rejected transition"));
    assert!(
        !shared
            .lock()
            .unwrap()
            .state()
            .runtime_completion_result_resolved
    );
    assert!(carrier.pending_nondirected_run_terminal().is_some());

    // The finalized rows must not take the ordinary already-finalized shortcut
    // through a degraded live shell. Retry preserves the carrier and all rows.
    let retry = resolve_machine_terminal_completion_waiters(
        &driver,
        None,
        gate.lock_owned().await,
        &carrier,
        &inputs,
        &run_id,
        FAILURE.to_owned(),
    )
    .await
    .unwrap_err();
    assert!(retry.to_string().contains("durability reload required"));
    assert_eq!(health.require_ready().unwrap_err(), required);
    assert!(carrier.pending_nondirected_run_terminal().is_some());
    let entry = driver.lock().await;
    assert!(matches!(
        entry.input_terminal_completion_batch_for_run(&run_id, &inputs),
        Err(crate::RuntimeDriverError::RecoveryRepairBlocked { .. })
    ));
    for (input_id, committed_row) in inputs.iter().zip(committed) {
        let live_row = entry.as_driver().stored_input_state(input_id).unwrap();
        assert_eq!(serde_json::to_vec(&live_row).unwrap(), committed_row);
        let durable_row = store
            .load_input_state(&runtime_id, input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_vec(&durable_row).unwrap(), committed_row);
    }
}

#[tokio::test]
async fn abandoned_completion_generated_authority_refuses_mixed_recipient_phases() {
    let fixture = Fixture::failed_batch().await;
    let before = fixture.later_attached().await;
    let entry = fixture.driver.lock().await;
    let first = entry
        .as_driver()
        .stored_input_state(&fixture.inputs[0])
        .unwrap();
    let owner_id = first.state.terminal_completion.unwrap().owner_input_id;
    let completion = entry
        .as_driver()
        .stored_input_state(&owner_id)
        .unwrap()
        .state
        .terminal_completion
        .unwrap();
    let other = fixture
        .inputs
        .iter()
        .find(|id| **id != owner_id)
        .unwrap()
        .to_string();
    for phase in [mm::InputPhase::Consumed, mm::InputPhase::Queued] {
        let mut state = before.clone();
        state.input_phases.insert(other.clone(), phase);
        if phase == mm::InputPhase::Consumed {
            state
                .input_terminal_kind
                .insert(other.clone(), mm::InputTerminalKind::Consumed);
        } else {
            state.input_terminal_kind.remove(&other);
            state.input_lane.insert(other.clone(), mm::InputLane::Queue);
        }
        let mut machine = mm::MeerkatMachineAuthority::recover_from_state(state).unwrap();
        let classification = mm::MeerkatMachineInput::ClassifyTerminalCompletionCorrelation {
            owner_input_id: owner_id.to_string(),
            run_id: Some(mm::RunId::from_domain(&fixture.run_id)),
            terminal: Some(mm::RuntimeCompletionTerminalObservation::MachineTerminal),
            recipient_input_ids: fixture.inputs.iter().map(ToString::to_string).collect(),
            terminal_outcome: Some(mm::TurnTerminalOutcome::Failed),
            terminal_cause_kind: Some(mm::TurnTerminalCauseKind::RuntimeApplyFailure),
            requires_session_checkpoint: false,
            has_interaction_terminal_outbox: false,
        };
        assert!(mm::MeerkatMachineMutator::apply(&mut machine, classification).is_err());
        let resolution = mm::MeerkatMachineInput::ResolveAbandonedCompletionResult {
            owner_input_id: owner_id.to_string(),
            run_id: mm::RunId::from_domain(&fixture.run_id),
            candidate_digest: completion.candidate_digest.clone(),
            completion_input_ids_digest: completion.completion_input_ids_digest.clone(),
            recipient_input_ids: fixture.inputs.iter().map(ToString::to_string).collect(),
            terminal_outcome: mm::TurnTerminalOutcome::Failed,
            terminal_cause_kind: mm::TurnTerminalCauseKind::RuntimeApplyFailure,
            requires_session_checkpoint: false,
            has_interaction_terminal_outbox: false,
            finalization: mm::RuntimeCompletionFinalizationObservation::Succeeded,
        };
        assert!(mm::MeerkatMachineMutator::apply(&mut machine, resolution).is_err());
        assert_live_run_unchanged(&before, machine.state());
    }
}

#[tokio::test]
async fn stage_refusal_completion_rejects_mixed_recipient_evidence() {
    let fixture = Fixture::failed_batch().await;
    let before = fixture.later_attached().await;
    let entry = fixture.driver.lock().await;
    let owner_id = entry
        .as_driver()
        .stored_input_state(&fixture.inputs[0])
        .unwrap()
        .state
        .terminal_completion
        .unwrap()
        .owner_input_id;
    let other = fixture
        .inputs
        .iter()
        .find(|id| **id != owner_id)
        .unwrap()
        .to_string();
    let receipt_run = mm::RunId::from_domain(&fixture.run_id);
    let prior_run = mm::RunId::from_domain(&RunId::new());
    // Exercise the ordinary-run fallback: the owner itself is never bound
    // to the receipt run, so another recipient must not bypass validation.
    for owner_prior_run in [None, Some(prior_run.clone())] {
        for consumed_recipient in [false, true] {
            let mut state = before.clone();
            for input_id in &fixture.inputs {
                state.input_run_associations.remove(&input_id.to_string());
            }
            if let Some(run_id) = owner_prior_run.as_ref() {
                state
                    .input_run_associations
                    .insert(owner_id.to_string(), run_id.clone());
            }
            if consumed_recipient {
                state
                    .input_phases
                    .insert(other.clone(), mm::InputPhase::Consumed);
                state
                    .input_terminal_kind
                    .insert(other.clone(), mm::InputTerminalKind::Consumed);
            } else {
                state
                    .input_run_associations
                    .insert(other.clone(), receipt_run.clone());
            }
            let mut machine = mm::MeerkatMachineAuthority::recover_from_state(state).unwrap();
            let classification = mm::MeerkatMachineInput::ClassifyTerminalCompletionCorrelation {
                owner_input_id: owner_id.to_string(),
                run_id: Some(receipt_run.clone()),
                terminal: Some(mm::RuntimeCompletionTerminalObservation::MachineTerminal),
                recipient_input_ids: fixture.inputs.iter().map(ToString::to_string).collect(),
                terminal_outcome: Some(mm::TurnTerminalOutcome::Failed),
                terminal_cause_kind: Some(mm::TurnTerminalCauseKind::RuntimeApplyFailure),
                requires_session_checkpoint: false,
                has_interaction_terminal_outbox: false,
            };
            assert!(
                mm::MeerkatMachineMutator::apply(&mut machine, classification).is_err(),
                "fallback accepted owner attribution {owner_prior_run:?}, consumed recipient {consumed_recipient}"
            );
            assert_live_run_unchanged(&before, machine.state());
        }
    }
}

#[tokio::test]
async fn stage_refusal_completion_accepts_independent_historical_attribution() {
    let fixture = Fixture::failed_batch().await;
    let before = fixture.later_attached().await;
    let entry = fixture.driver.lock().await;
    let owner_id = entry
        .as_driver()
        .stored_input_state(&fixture.inputs[0])
        .unwrap()
        .state
        .terminal_completion
        .unwrap()
        .owner_input_id;
    let other = fixture
        .inputs
        .iter()
        .find(|id| **id != owner_id)
        .unwrap()
        .to_string();
    let receipt_run = mm::RunId::from_domain(&fixture.run_id);
    let owner_prior_run = mm::RunId::from_domain(&RunId::new());
    let other_prior_run = mm::RunId::from_domain(&RunId::new());
    for owner_attribution in [None, Some(owner_prior_run)] {
        for other_attribution in [None, Some(other_prior_run.clone())] {
            let mut state = before.clone();
            for (input_id, attribution) in [
                (owner_id.to_string(), owner_attribution.as_ref()),
                (other.clone(), other_attribution.as_ref()),
            ] {
                state.input_run_associations.remove(&input_id);
                if let Some(run_id) = attribution {
                    state
                        .input_run_associations
                        .insert(input_id, run_id.clone());
                }
            }
            let mut machine = mm::MeerkatMachineAuthority::recover_from_state(state).unwrap();
            let classification = mm::MeerkatMachineInput::ClassifyTerminalCompletionCorrelation {
                owner_input_id: owner_id.to_string(),
                run_id: Some(receipt_run.clone()),
                terminal: Some(mm::RuntimeCompletionTerminalObservation::MachineTerminal),
                recipient_input_ids: fixture.inputs.iter().map(ToString::to_string).collect(),
                terminal_outcome: Some(mm::TurnTerminalOutcome::Failed),
                terminal_cause_kind: Some(mm::TurnTerminalCauseKind::RuntimeApplyFailure),
                requires_session_checkpoint: false,
                has_interaction_terminal_outbox: false,
            };
            let transition =
                mm::MeerkatMachineMutator::apply(&mut machine, classification).unwrap();
            assert!(matches!(
                transition.effects(),
                [mm::MeerkatMachineEffect::TerminalCompletionCorrelationClassified {
                    owner_input_id,
                    run_id: Some(run_id),
                    correlation: mm::TerminalCompletionCorrelation::Run,
                }] if owner_input_id == &owner_id.to_string() && run_id == &receipt_run
            ));
            assert_live_run_unchanged(&before, machine.state());
        }
    }
}

#[tokio::test]
async fn multiple_abandoned_batches_recover_without_replacing_newer_run() {
    let mut fixture = Fixture::failed_batch().await;
    let first_inputs = fixture.inputs.clone();
    let first_run = fixture.run_id.clone();
    let (second_inputs, second_run, _second_carrier) = Fixture::fail_another_batch(
        &fixture.driver,
        CoreApplyFailureCause::executor_internal(FAILURE),
    )
    .await;
    assert_ne!(first_run, second_run);
    let before = fixture.later_attached().await;
    let batches = fixture
        .driver
        .lock()
        .await
        .input_terminal_completion_recovery_batches()
        .await
        .unwrap();
    assert_eq!(batches.len(), 2);
    assert!(
        batches
            .iter()
            .all(|batch| batch.correlation == mm::TerminalCompletionCorrelation::AbandonedInput)
    );
    drain_recovered_input_terminal_completions(&fixture.driver, None, &mut NoExecution)
        .await
        .unwrap();
    fixture.finalized().await;
    fixture.inputs = second_inputs;
    fixture.run_id = second_run;
    fixture.finalized().await;
    let all_inputs = first_inputs
        .into_iter()
        .chain(fixture.inputs.iter().cloned())
        .collect::<Vec<_>>();
    let raw = all_inputs
        .iter()
        .map(|id| fixture.raw_row(id))
        .collect::<Vec<_>>();
    let shared = fixture.driver.lock().await.shared_dsl_authority();
    assert_live_run_unchanged(&before, shared.lock().unwrap().state());
    drain_recovered_input_terminal_completions(&fixture.driver, None, &mut NoExecution)
        .await
        .unwrap();
    assert_eq!(
        raw,
        all_inputs
            .iter()
            .map(|id| fixture.raw_row(id))
            .collect::<Vec<_>>()
    );
    assert_live_run_unchanged(&before, shared.lock().unwrap().state());
}

#[tokio::test]
async fn abandoned_completion_uses_its_receipt_when_a_later_run_keeps_completed_state() {
    let fixture = Fixture::failed_batch().await;
    let completed_run = RunId::new();
    let later_failed_run = RunId::new();
    let before = {
        let mut entry = fixture.driver.lock().await;
        owner::machine_begin_run(&mut entry, completed_run.clone()).unwrap();
        let shared = entry.shared_dsl_authority();
        let mut machine = shared.lock().unwrap();
        let session_id = machine.state().session_id.clone().unwrap();
        for input in [
            mm::MeerkatMachineInput::StartConversationRun {
                run_id: mm::RunId::from_domain(&completed_run),
                primitive_kind: mm::TurnPrimitiveKind::ConversationTurn,
                admitted_content_shape: mm::ContentShape::Conversation,
                vision_enabled: false,
                image_tool_results_enabled: false,
                max_extraction_retries: 0,
            },
            mm::MeerkatMachineInput::PrimitiveApplied {
                run_id: mm::RunId::from_domain(&completed_run),
            },
            mm::MeerkatMachineInput::RunCompleted {
                run_id: mm::RunId::from_domain(&completed_run),
            },
            mm::MeerkatMachineInput::Commit {
                input_id: mm::InputId::from_domain(&InputId::new()),
                run_id: mm::RunId::from_domain(&completed_run),
            },
            mm::MeerkatMachineInput::Prepare {
                session_id,
                run_id: mm::RunId::from_domain(&later_failed_run),
            },
            mm::MeerkatMachineInput::RunFailed {
                run_id: mm::RunId::from_domain(&later_failed_run),
                runtime_apply_failure_cause: Some(mm::RuntimeApplyFailureCause::RuntimeTurn),
                runtime_apply_failure_message: Some("a different later failure".into()),
                machine_terminal_failure_observed: false,
                terminal_failure_source: None,
                error: "a different later failure".into(),
            },
        ] {
            mm::MeerkatMachineMutator::apply(&mut *machine, input).unwrap();
        }
        let before = machine.state().clone();
        assert_eq!(
            before.terminal_outcome,
            Some(mm::TurnTerminalOutcome::Completed)
        );
        assert_eq!(
            before.current_run_id,
            Some(mm::RunId::from_domain(&later_failed_run))
        );
        drop(machine);
        if let owner::DriverEntry::Persistent(driver) = &mut *entry {
            driver
                .inner_mut()
                .sync_control_projection_from_dsl_authority();
        }
        before
    };
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    resolve_machine_terminal_completion_waiters(
        &fixture.driver,
        None,
        gate.lock_owned().await,
        &fixture.carrier,
        &fixture.inputs,
        &fixture.run_id,
        FAILURE.to_owned(),
    )
    .await
    .unwrap();
    fixture.finalized().await;
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    let shared = fixture.driver.lock().await.shared_dsl_authority();
    assert_live_run_unchanged(&before, shared.lock().unwrap().state());
}

#[tokio::test]
async fn abandoned_completion_keeps_its_failure_beside_later_cancelled_and_llm_failed_runs() {
    for cancelled in [true, false] {
        let fixture = Fixture::failed_batch().await;
        let later = RunId::new();
        let before = {
            let mut entry = fixture.driver.lock().await;
            owner::machine_begin_run(&mut entry, later.clone()).unwrap();
            let shared = entry.shared_dsl_authority();
            let mut machine = shared.lock().unwrap();
            mm::MeerkatMachineMutator::apply(
                &mut *machine,
                mm::MeerkatMachineInput::StartConversationRun {
                    run_id: mm::RunId::from_domain(&later),
                    primitive_kind: mm::TurnPrimitiveKind::ConversationTurn,
                    admitted_content_shape: mm::ContentShape::Conversation,
                    vision_enabled: false,
                    image_tool_results_enabled: false,
                    max_extraction_retries: 0,
                },
            )
            .unwrap();
            let terminal = if cancelled {
                mm::MeerkatMachineInput::RunCancelled {
                    run_id: mm::RunId::from_domain(&later),
                }
            } else {
                mm::MeerkatMachineInput::RunFailed {
                    run_id: mm::RunId::from_domain(&later),
                    runtime_apply_failure_cause: None,
                    runtime_apply_failure_message: None,
                    machine_terminal_failure_observed: false,
                    terminal_failure_source: Some(mm::RunFailureSourceKind::Llm),
                    error: "the later model failed independently".into(),
                }
            };
            mm::MeerkatMachineMutator::apply(&mut *machine, terminal).unwrap();
            let before = machine.state().clone();
            assert_eq!(
                before.terminal_outcome,
                Some(if cancelled {
                    mm::TurnTerminalOutcome::Cancelled
                } else {
                    mm::TurnTerminalOutcome::Failed
                })
            );
            assert_eq!(
                before.terminal_cause_kind,
                if cancelled {
                    None
                } else {
                    Some(mm::TurnTerminalCauseKind::LlmFailure)
                }
            );
            before
        };
        let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
        resolve_machine_terminal_completion_waiters(
            &fixture.driver,
            None,
            gate.lock_owned().await,
            &fixture.carrier,
            &fixture.inputs,
            &fixture.run_id,
            FAILURE.to_owned(),
        )
        .await
        .unwrap();
        fixture.finalized().await;
        assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
        let shared = fixture.driver.lock().await.shared_dsl_authority();
        assert_live_run_unchanged(&before, shared.lock().unwrap().state());
    }
}

#[tokio::test]
async fn retained_live_boundary_join_without_observers_finalizes_before_return() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(SqliteRuntimeStore::new(dir.path().join("runtime.sqlite")).unwrap());
    let session_id = SessionId::new();
    let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
    let driver = Arc::new(crate::tokio::sync::Mutex::new(
        owner::DriverEntry::Persistent(PersistentRuntimeDriver::new(
            runtime_id.clone(),
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )),
    ));
    let batch_input = Input::Prompt(PromptInput::new("keep a batch run active", None));
    assert!(
        driver
            .lock()
            .await
            .as_driver_mut()
            .accept_input(batch_input)
            .await
            .unwrap()
            .is_accepted()
    );
    let batch = owner::machine_authorize_runtime_loop_batch(&*driver.lock().await).unwrap();
    let run_id = RunId::new();
    assert!(matches!(
        owner::prepare_runtime_loop_batch_start(&driver, run_id.clone(), batch)
            .await
            .unwrap(),
        owner::RuntimeLoopBatchStart::Started
    ));
    let shared = driver.lock().await.shared_dsl_authority();
    mm::MeerkatMachineMutator::apply(
        &mut *shared.lock().unwrap(),
        mm::MeerkatMachineInput::StartConversationRun {
            run_id: mm::RunId::from_domain(&run_id),
            primitive_kind: mm::TurnPrimitiveKind::ConversationTurn,
            admitted_content_shape: mm::ContentShape::Conversation,
            vision_enabled: false,
            image_tool_results_enabled: false,
            max_extraction_retries: 0,
        },
    )
    .unwrap();
    mm::MeerkatMachineMutator::apply(
        &mut *shared.lock().unwrap(),
        mm::MeerkatMachineInput::PrimitiveApplied {
            run_id: mm::RunId::from_domain(&run_id),
        },
    )
    .unwrap();

    let append = meerkat_core::lifecycle::ConversationAppend {
        runtime_source: None,
        role: meerkat_core::lifecycle::ConversationAppendRole::SystemNotice,
        content: meerkat_core::lifecycle::CoreRenderable::SystemNotice {
            kind: meerkat_core::types::SystemNoticeKind::Generic,
            body: Some("retained durable append without observers".into()),
            blocks: Vec::new(),
        },
        identity: None,
    };
    let mut steer = PromptInput::new(
        "",
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Steer),
                ..Default::default()
            },
        ),
    );
    steer.typed_turn_appends = vec![append.clone()];
    let input = Input::Prompt(steer);
    let input_id = input.id().clone();
    assert!(
        driver
            .lock()
            .await
            .as_driver_mut()
            .accept_input(input)
            .await
            .unwrap()
            .is_accepted()
    );

    let state = meerkat_core::TransientTurnContextStateHandle::new();
    let run_guard = state.begin_boundary_run_for_test(run_id.clone()).unwrap();
    let delivery = meerkat_core::TurnBoundaryDelivery::DurableAppends(
        meerkat_core::DurableTurnBoundaryAppends::try_new(input_id.clone(), vec![append], None)
            .unwrap(),
    );
    let prepare_state = state.clone();
    let prepare_run = run_id.clone();
    let preparation = tokio::spawn(async move {
        prepare_state
            .prepare_active_turn_boundary(&prepare_run, delivery)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !state.has_waiting_delivery_for_test() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let take_state = state.clone();
    let take_run = run_id.clone();
    let taken =
        tokio::spawn(async move { take_state.take_boundary_for_test(&take_run, true).await });
    let stage = tokio::time::timeout(std::time::Duration::from_secs(5), preparation)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .into_stage_output(None);
    let witness = stage.delivery_witness().cloned().unwrap();
    assert!(matches!(
        driver
            .lock()
            .await
            .machine_realize_live_boundary_durable_append_joined(
                &run_id,
                &input_id,
                witness.clone(),
            )
            .await
            .unwrap(),
        owner::LiveBoundaryJoinOutcome::Joined
    ));
    stage.commit().unwrap();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), taken)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .applied_durable
            .unwrap()
            .input_id(),
        &input_id
    );
    drop(run_guard);
    assert_eq!(
        witness.outcome(),
        meerkat_core::CoreBoundaryDeliveryOutcome::Applied
    );
    let resolution =
        resolve_live_boundary_joins_for_terminal(&driver, &mut NoExecution, &session_id, &run_id)
            .await
            .unwrap();
    assert_eq!(resolution.retained, vec![input_id.clone()]);

    consume_retained_live_boundary_joins_without_commit(
        &driver,
        None,
        &run_id,
        &resolution.retained,
        &session_id,
    )
    .await
    .unwrap();
    let stored = store
        .load_input_state(&runtime_id, &input_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.seed.phase,
        crate::input_state::InputLifecycleState::Consumed
    );
    assert!(matches!(
        stored.state.terminal_completion.unwrap().phase,
        InputTerminalCompletionPhase::Finalized { .. }
    ));
}
