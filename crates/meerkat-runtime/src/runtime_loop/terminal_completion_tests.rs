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
