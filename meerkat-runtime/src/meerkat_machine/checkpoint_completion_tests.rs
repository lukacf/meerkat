#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::input::{InputOrigin, PeerConvention, PeerInput, PromptInput};
use crate::input_state::{InputTerminalCompletionPhase, InteractionTerminalCandidate};
use crate::meerkat_machine::dsl as mm;
use crate::store::{RuntimeStore, SerializedSessionSnapshot, SqliteRuntimeStore};
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::types::{HandlingMode, Message, UserMessage};

struct NoExecution;

#[async_trait::async_trait]
impl CoreExecutor for NoExecution {
    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        panic!("checkpoint receipt recovery must not cancel a run")
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        panic!("checkpoint receipt recovery must not stop a run")
    }

    async fn apply(
        &mut self,
        _run_id: RunId,
        _primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        panic!("recovering a consumed checkpoint must not execute a turn")
    }

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        _snapshot: Arc<Vec<u8>>,
    ) -> Result<(), CoreExecutorError> {
        panic!("the retained checkpoint requires no session checkpoint")
    }

    async fn publish_interaction_terminals(
        &mut self,
        _events: &[meerkat_core::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        panic!("non-directed checkpoint recovery must not invent a publication")
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    store: Arc<SqliteRuntimeStore>,
    driver: SharedDriver,
    session_id: SessionId,
    run_id: RunId,
    transcript: Arc<Vec<u8>>,
    authority: crate::store::WholeBlobStoreAuthority,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteRuntimeStore::new(dir.path().join("runtime.sqlite")).unwrap());
        let session_id = SessionId::parse("01a000bb-b69e-7570-933d-ffd5d61d51ee").unwrap();
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        let mut session = meerkat_core::Session::with_id(session_id.clone());
        session.push(Message::User(UserMessage::text("retained history")));
        let transcript = Arc::new(serde_json::to_vec(&session).unwrap());
        store
            .commit_session_snapshot(
                &runtime_id,
                SerializedSessionSnapshot {
                    session_snapshot: Arc::clone(&transcript),
                },
            )
            .await
            .unwrap();
        let authority = store
            .load_whole_blob_store_authority(&runtime_id)
            .await
            .unwrap()
            .unwrap();
        let mut driver = PersistentRuntimeDriver::new(
            runtime_id,
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        let main = Input::Prompt(PromptInput::new("run in progress", None));
        let main_id = main.id().clone();
        driver.accept_input(main).await.unwrap();
        let run_id = RunId::from_uuid(
            uuid::Uuid::parse_str("01a0826a-f847-7ef2-8753-400d56761ce1").unwrap(),
        );
        driver.contract_begin_run_authority(run_id.clone()).unwrap();
        driver
            .machine_realize_authorized_stage_batch(test_authorized_stage_for_run(
                vec![main_id],
                run_id.clone(),
            ))
            .unwrap();
        Self {
            _dir: dir,
            store,
            driver: Arc::new(Mutex::new(DriverEntry::Persistent(driver))),
            session_id,
            run_id,
            transcript,
            authority,
        }
    }

    fn runtime_id(&self) -> LogicalRuntimeId {
        LogicalRuntimeId::for_session(&self.session_id)
    }

    async fn checkpoint(&self, input_id: InputId) {
        self.checkpoint_in_run(input_id, &self.run_id).await;
    }

    async fn checkpoint_in_run(&self, input_id: InputId, run_id: &RunId) {
        let mut header = PromptInput::new("", None).header;
        header.id = input_id.clone();
        header.timestamp = chrono::DateTime::parse_from_rfc3339("2026-09-08T19:09:09.690958Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        header.source = InputOrigin::Peer {
            peer_id: "checkpoint-peer".into(),
            display_identity: None,
            runtime_id: None,
        };
        let input = Input::Peer(PeerInput {
            header,
            directed_interaction_id: None,
            convention: Some(PeerConvention::Message),
            content: "already consumed steer".into(),
            payload: None,
            handling_mode: Some(HandlingMode::Steer),
            sender_taint: None,
            objective_id: None,
            system_prompts: Vec::new(),
            injected_context: Vec::new(),
        });
        let mut entry = self.driver.lock().await;
        let admission = entry
            .resolve_admission_with_active_turn_boundary(&input, true)
            .unwrap();
        entry.accept_resolved_input(input, admission).await.unwrap();
        entry
            .machine_realize_live_boundary_context_injected(
                run_id,
                std::slice::from_ref(&input_id),
                None,
                &self.session_id,
            )
            .await
            .unwrap();
    }

    async fn pending(&self, input_id: &InputId) -> StoredInputState {
        let row = self
            .store
            .load_input_state(&self.runtime_id(), input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            row.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        let completion = row.state.terminal_completion.as_ref().unwrap();
        assert_eq!(completion.owner_input_id, *input_id);
        assert!(!completion.requires_session_checkpoint);
        assert!(matches!(
            completion.candidate,
            Some(InteractionTerminalCandidate::CompletedWithoutResult)
        ));
        assert!(matches!(
            completion.phase,
            InputTerminalCompletionPhase::Pending
        ));
        assert!(row.state.interaction_terminal_outbox.is_none());
        row
    }

    fn raw_row(&self, input_id: &InputId) -> Vec<u8> {
        let connection = rusqlite::Connection::open(self.store.path()).unwrap();
        connection
            .query_row(
                "SELECT state_json FROM runtime_input_states WHERE runtime_id=?1 AND input_id=?2",
                rusqlite::params![self.runtime_id().to_string(), input_id.to_string()],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn replace_fixture_row(&self, input_id: &InputId, bytes: &[u8]) {
        rusqlite::Connection::open(self.store.path())
            .unwrap()
            .execute(
                "UPDATE runtime_input_states SET state_json=?3 WHERE runtime_id=?1 AND input_id=?2",
                rusqlite::params![self.runtime_id().to_string(), input_id.to_string(), bytes],
            )
            .unwrap();
    }

    async fn fresh_driver_from_durable(&self) -> Result<SharedDriver, RuntimeDriverError> {
        let mut driver = PersistentRuntimeDriver::new(
            self.runtime_id(),
            self.store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        driver.inner_mut().ensure_contract_session_authority()?;
        driver.recover_inputs_after_runtime_authority(None).await?;
        Ok(Arc::new(Mutex::new(DriverEntry::Persistent(driver))))
    }

    async fn assert_finalized(&self, input_id: &InputId) {
        let row = self
            .store
            .load_input_state(&self.runtime_id(), input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        let completion = row.state.terminal_completion.unwrap();
        assert!(matches!(
            completion.phase,
            InputTerminalCompletionPhase::Finalized { .. }
        ));
        assert!(matches!(
            completion.outcome,
            Some(crate::completion::CompletionOutcome::CompletedWithoutResult)
        ));
        let conn = rusqlite::Connection::open(self.store.path()).unwrap();
        let pending: usize = conn.query_row(
            "SELECT count(*) FROM runtime_pending_terminal_owners WHERE runtime_id=?1 AND owner_input_id=?2",
            rusqlite::params![self.runtime_id().to_string(), input_id.to_string()],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(pending, 0);
        assert_eq!(
            self.store
                .load_session_snapshot(&self.runtime_id())
                .await
                .unwrap()
                .unwrap(),
            self.transcript,
        );
        assert_eq!(
            self.store
                .load_whole_blob_store_authority(&self.runtime_id())
                .await
                .unwrap()
                .as_ref(),
            Some(&self.authority),
        );
    }
}

fn old_input_id() -> InputId {
    InputId::from_uuid(uuid::Uuid::parse_str("01a0826c-96ba-7b53-8706-6352bdbe76d0").unwrap())
}

async fn newer_run_owns_correlation(fixture: &Fixture) -> mm::MeerkatMachineState {
    let mut entry = fixture.driver.lock().await;
    let shared = entry.shared_dsl_authority();
    let mut machine = shared.lock().unwrap();
    mm::MeerkatMachineMutator::apply(
        &mut *machine,
        mm::MeerkatMachineInput::RunCompleted {
            run_id: mm::RunId::from_domain(&fixture.run_id),
        },
    )
    .unwrap();
    mm::MeerkatMachineMutator::apply(
        &mut *machine,
        mm::MeerkatMachineInput::ServiceTurnCommitted {
            run_id: mm::RunId::from_domain(&fixture.run_id),
        },
    )
    .unwrap();
    let newer = RunId::new();
    mm::MeerkatMachineMutator::apply(
        &mut *machine,
        mm::MeerkatMachineInput::Prepare {
            session_id: mm::SessionId::from_domain(&fixture.session_id),
            run_id: mm::RunId::from_domain(&newer),
        },
    )
    .unwrap();
    mm::MeerkatMachineMutator::apply(
        &mut *machine,
        mm::MeerkatMachineInput::RunCompleted {
            run_id: mm::RunId::from_domain(&newer),
        },
    )
    .unwrap();
    let state = machine.state().clone();
    assert_eq!(state.current_run_id, Some(mm::RunId::from_domain(&newer)));
    assert_eq!(state.runtime_completion_result_run_id, state.current_run_id);
    drop(machine);
    if let DriverEntry::Persistent(driver) = &mut *entry {
        driver
            .inner_mut()
            .sync_control_projection_from_dsl_authority();
    }
    state
}

fn assert_run_unchanged(before: &mm::MeerkatMachineState, after: &mm::MeerkatMachineState) {
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
async fn checkpoint_interruption_retains_exact_input_correlation_and_recovers_under_newer_run() {
    let fixture = Arc::new(Fixture::new().await);
    let input_id = old_input_id();
    let before = fixture
        .driver
        .lock()
        .await
        .shared_dsl_authority()
        .lock()
        .unwrap()
        .state()
        .clone();
    let (committed_tx, committed_rx) = tokio::sync::oneshot::channel();
    let worker = tokio::spawn({
        let fixture = fixture.clone();
        let input_id = input_id.clone();
        async move {
            fixture.checkpoint(input_id).await;
            committed_tx.send(()).unwrap();
            // The exact production producer has committed; its caller has not
            // reached completion finalization when it is interrupted.
            std::future::pending::<()>().await;
        }
    });
    committed_rx.await.unwrap();
    worker.abort();
    assert!(worker.await.unwrap_err().is_cancelled());
    fixture.pending(&input_id).await;
    let after = fixture
        .driver
        .lock()
        .await
        .shared_dsl_authority()
        .lock()
        .unwrap()
        .state()
        .clone();
    assert_run_unchanged(&before, &after);
    let newer = newer_run_owns_correlation(&fixture).await;
    let old_path_error = machine_recover_runtime_completion_result_correlation(
        &*fixture.driver.lock().await,
        &fixture.run_id,
        crate::input_state::RuntimeCompletionTerminalRecovery::NonMachine,
    )
    .expect_err("the released single-slot recovery still rejects this exact older batch");
    assert!(
        old_path_error
            .to_string()
            .contains("RecoverRuntimeCompletionResultCorrelation")
    );
    eprintln!("released checkpoint recovery obstruction reproduced: {old_path_error}");
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(
        &fixture.driver,
        &mut NoExecution,
    )
    .await
    .unwrap();
    fixture.assert_finalized(&input_id).await;
    let after = fixture
        .driver
        .lock()
        .await
        .shared_dsl_authority()
        .lock()
        .unwrap()
        .state()
        .clone();
    assert_run_unchanged(&newer, &after);
    let exact_receipt = fixture.raw_row(&input_id);
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(
        &fixture.driver,
        &mut NoExecution,
    )
    .await
    .unwrap();
    assert_eq!(exact_receipt, fixture.raw_row(&input_id));
}

#[tokio::test]
async fn multiple_checkpoint_batches_of_one_run_settle_independently() {
    let fixture = Fixture::new().await;
    let inputs = [old_input_id(), InputId::new()];
    for input_id in &inputs {
        fixture.checkpoint(input_id.clone()).await;
        fixture.pending(input_id).await;
    }
    let newer = newer_run_owns_correlation(&fixture).await;
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(
        &fixture.driver,
        &mut NoExecution,
    )
    .await
    .unwrap();
    for input_id in &inputs {
        fixture.assert_finalized(input_id).await;
    }
    let after = fixture
        .driver
        .lock()
        .await
        .shared_dsl_authority()
        .lock()
        .unwrap()
        .state()
        .clone();
    assert_run_unchanged(&newer, &after);
}

#[tokio::test]
async fn homecore_consumed_checkpoint_recovers_from_attached_without_replacing_later_correlation() {
    // Reconstructs the confirmed frozen checkpoint-owner/receipt shape and
    // Attached refusal, not the historical production cancellation timing.
    let fixture = Fixture::new().await;
    let input_id = old_input_id();
    fixture.checkpoint(input_id.clone()).await;
    let newer = newer_run_owns_correlation(&fixture).await;
    let before = {
        let mut entry = fixture.driver.lock().await;
        let shared = entry.shared_dsl_authority();
        let mut machine = shared.lock().unwrap();
        mm::MeerkatMachineMutator::apply(
            &mut *machine,
            mm::MeerkatMachineInput::ServiceTurnCommitted {
                run_id: newer.current_run_id.clone().unwrap(),
            },
        )
        .unwrap();
        mm::MeerkatMachineMutator::apply(
            &mut *machine,
            mm::MeerkatMachineInput::PrepareBindings {
                session_id: mm::SessionId::from_domain(&fixture.session_id),
                agent_runtime_id: mm::AgentRuntimeId::from("checkpoint-recovery-fixture"),
                fence_token: mm::FenceToken::from(1),
                generation: Some(mm::Generation::from(0)),
                runtime_epoch_id: None,
            },
        )
        .unwrap();
        let state = machine.state().clone();
        drop(machine);
        if let DriverEntry::Persistent(driver) = &mut *entry {
            driver
                .inner_mut()
                .sync_control_projection_from_dsl_authority();
        }
        state
    };
    let error = machine_recover_runtime_completion_result_correlation(
        &*fixture.driver.lock().await,
        &fixture.run_id,
        crate::input_state::RuntimeCompletionTerminalRecovery::NonMachine,
    )
    .expect_err("released recovery must reproduce the original Attached guard refusal");
    assert!(
        error
            .to_string()
            .contains("guard rejected transition from Attached"),
        "{error}"
    );
    eprintln!("original Attached checkpoint recovery obstruction reproduced: {error}");
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(
        &fixture.driver,
        &mut NoExecution,
    )
    .await
    .unwrap();
    fixture.assert_finalized(&input_id).await;
    let after = fixture
        .driver
        .lock()
        .await
        .shared_dsl_authority()
        .lock()
        .unwrap()
        .state()
        .clone();
    assert_run_unchanged(&before, &after);
}

#[tokio::test]
async fn older_and_current_run_checkpoint_batches_do_not_compete_for_one_slot() {
    let fixture = Fixture::new().await;
    let old = old_input_id();
    fixture.checkpoint(old.clone()).await;
    let newer = RunId::new();
    {
        let mut entry = fixture.driver.lock().await;
        let shared = entry.shared_dsl_authority();
        let mut authority = shared.lock().unwrap();
        for input in [
            mm::MeerkatMachineInput::RunCompleted {
                run_id: mm::RunId::from_domain(&fixture.run_id),
            },
            mm::MeerkatMachineInput::ServiceTurnCommitted {
                run_id: mm::RunId::from_domain(&fixture.run_id),
            },
            mm::MeerkatMachineInput::Prepare {
                session_id: mm::SessionId::from_domain(&fixture.session_id),
                run_id: mm::RunId::from_domain(&newer),
            },
        ] {
            mm::MeerkatMachineMutator::apply(&mut *authority, input).unwrap();
        }
        drop(authority);
        if let DriverEntry::Persistent(driver) = &mut *entry {
            driver
                .inner_mut()
                .sync_control_projection_from_dsl_authority();
        }
    }
    let current = InputId::new();
    fixture.checkpoint_in_run(current.clone(), &newer).await;
    let before = fixture
        .driver
        .lock()
        .await
        .shared_dsl_authority()
        .lock()
        .unwrap()
        .state()
        .clone();
    assert_eq!(before.current_run_id, Some(mm::RunId::from_domain(&newer)));
    for input_id in [&old, &current] {
        fixture.pending(input_id).await;
    }
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(
        &fixture.driver,
        &mut NoExecution,
    )
    .await
    .unwrap();
    for input_id in [&old, &current] {
        fixture.assert_finalized(input_id).await;
    }
    let after = fixture
        .driver
        .lock()
        .await
        .shared_dsl_authority()
        .lock()
        .unwrap()
        .state()
        .clone();
    assert_run_unchanged(&before, &after);
}

#[tokio::test]
async fn checkpoint_recovery_after_restart_settles_then_releases_exact_materialization_owner() {
    let fixture = Fixture::new().await;
    let input_id = old_input_id();
    fixture.checkpoint(input_id.clone()).await;
    let machine = Arc::new(super::super::MeerkatMachine::persistent(
        fixture.store.clone(),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    let mut prepared = machine
        .prepare_session_materialization(fixture.session_id.clone())
        .await
        .unwrap();
    let recovered = machine
        .sessions
        .read()
        .await
        .get(&fixture.session_id)
        .unwrap()
        .driver
        .clone();
    let mutation = machine
        .lock_current_session_driver_gate(&fixture.session_id, &recovered)
        .await
        .unwrap();
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(
        &recovered,
        &mut NoExecution,
    )
    .await
    .unwrap();
    drop(mutation);
    fixture.assert_finalized(&input_id).await;
    prepared.rollback_now().await.unwrap();
    assert!(!machine.contains_session(&fixture.session_id).await);
    assert!(
        fixture
            .store
            .load_ops_lifecycle(&fixture.runtime_id())
            .await
            .unwrap()
            .is_none()
    );
    let mut successor = machine
        .prepare_session_materialization(fixture.session_id.clone())
        .await
        .expect("settled cleanup must not leave an active materialization claim");
    successor.rollback_now().await.unwrap();
}

#[tokio::test]
async fn checkpoint_recovery_refuses_wrong_durable_evidence_without_mutating_it() {
    for defect in [
        "run",
        "owner",
        "recipients",
        "candidate",
        "coherent_candidate",
        "digest",
    ] {
        let fixture = Fixture::new().await;
        let input_id = old_input_id();
        fixture.checkpoint(input_id.clone()).await;
        let mut row = fixture.pending(&input_id).await;
        let completion = row.state.terminal_completion.as_mut().unwrap();
        match defect {
            "run" => {
                completion.batch_key = crate::input_state::InputTerminalCompletionBatchKey::Run {
                    run_id: RunId::new(),
                }
            }
            "owner" => completion.owner_input_id = InputId::new(),
            "recipients" => {
                let recipients = completion.completion_input_ids.as_mut().unwrap();
                recipients.push(InputId::new());
                completion.completion_input_ids_digest =
                    crate::input_state::interaction_terminal_payload_digest(recipients).unwrap();
            }
            "candidate" => completion.candidate = Some(InteractionTerminalCandidate::Cancelled),
            "coherent_candidate" => {
                completion.candidate = Some(InteractionTerminalCandidate::RunResult {
                    result: Box::new(meerkat_core::types::RunResult {
                        text: "not a checkpoint outcome".to_string(),
                        session_id: fixture.session_id.clone(),
                        usage: Default::default(),
                        turns: 1,
                        tool_calls: 0,
                        terminal_cause_kind: None,
                        structured_output: None,
                        extraction_error: None,
                        schema_warnings: None,
                        skill_diagnostics: None,
                    }),
                });
                completion.candidate_digest =
                    crate::input_state::interaction_terminal_payload_digest(
                        completion.candidate.as_ref().unwrap(),
                    )
                    .unwrap();
            }
            "digest" => completion.candidate_digest = "sha256:wrong".to_string(),
            _ => unreachable!(),
        }
        let bytes = serde_json::to_vec(&row).unwrap();
        fixture.replace_fixture_row(&input_id, &bytes);
        let fresh = fixture.fresh_driver_from_durable().await;
        if defect == "coherent_candidate" {
            let fresh =
                fresh.expect("coherently encoded wrong result must reach semantic classification");
            let error = crate::runtime_loop::test_drain_recovered_input_terminal_completions(
                &fresh,
                &mut NoExecution,
            )
            .await
            .expect_err("an inline checkpoint cannot be reclassified as a full RunResult");
            assert!(
                error.contains("ClassifyTerminalCompletionCorrelation"),
                "{error}"
            );
        } else if let Ok(fresh) = fresh {
            assert!(
                crate::runtime_loop::test_drain_recovered_input_terminal_completions(
                    &fresh,
                    &mut NoExecution,
                )
                .await
                .is_err(),
                "{defect}",
            );
        }
        assert_eq!(bytes, fixture.raw_row(&input_id), "{defect}");
    }
}

#[tokio::test]
async fn checkpoint_recovery_refuses_mismatched_committed_boundary_receipts() {
    for defect in ["run", "sequence", "recipient"] {
        let fixture = Fixture::new().await;
        let input_id = old_input_id();
        fixture.checkpoint(input_id.clone()).await;
        let input_before = fixture.raw_row(&input_id);
        let stored = fixture.pending(&input_id).await;
        let sequence = stored.seed.last_boundary_sequence.unwrap();
        let mut receipt = fixture
            .store
            .load_boundary_receipt(&fixture.runtime_id(), &fixture.run_id, sequence)
            .await
            .unwrap()
            .unwrap();
        match defect {
            "run" => receipt.run_id = RunId::new(),
            "sequence" => receipt.sequence += 1,
            "recipient" => receipt.contributing_input_ids = vec![InputId::new()],
            _ => unreachable!(),
        }
        let bytes = serde_json::to_vec(&receipt).unwrap();
        let connection = rusqlite::Connection::open(fixture.store.path()).unwrap();
        connection
            .execute(
                "UPDATE runtime_boundary_receipts SET receipt_json=?4
             WHERE runtime_id=?1 AND run_id=?2 AND sequence=?3",
                rusqlite::params![
                    fixture.runtime_id().to_string(),
                    fixture.run_id.to_string(),
                    i64::try_from(sequence).unwrap(),
                    &bytes,
                ],
            )
            .unwrap();
        if let Ok(fresh) = fixture.fresh_driver_from_durable().await {
            assert!(
                crate::runtime_loop::test_drain_recovered_input_terminal_completions(
                    &fresh,
                    &mut NoExecution,
                )
                .await
                .is_err(),
                "{defect}"
            );
        }
        assert_eq!(fixture.raw_row(&input_id), input_before);
        let after: Vec<u8> = connection
            .query_row(
                "SELECT receipt_json FROM runtime_boundary_receipts
             WHERE runtime_id=?1 AND run_id=?2 AND sequence=?3",
                rusqlite::params![
                    fixture.runtime_id().to_string(),
                    fixture.run_id.to_string(),
                    i64::try_from(sequence).unwrap(),
                ],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bytes, after);
    }
}

#[tokio::test]
async fn checkpoint_receipt_cas_failure_keeps_consumed_candidate_for_cold_retry() {
    let fixture = Fixture::new().await;
    let input_id = old_input_id();
    fixture.checkpoint(input_id.clone()).await;
    let before = fixture.raw_row(&input_id);
    let conn = rusqlite::Connection::open(fixture.store.path()).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER reject_checkpoint_receipt BEFORE UPDATE ON runtime_input_states
         BEGIN SELECT RAISE(ABORT, 'injected checkpoint receipt failure'); END;",
    )
    .unwrap();
    assert!(
        crate::runtime_loop::test_drain_recovered_input_terminal_completions(
            &fixture.driver,
            &mut NoExecution,
        )
        .await
        .is_err()
    );
    assert_eq!(before, fixture.raw_row(&input_id));
    conn.execute_batch("DROP TRIGGER reject_checkpoint_receipt")
        .unwrap();
    drop(conn);
    let machine = Arc::new(super::super::MeerkatMachine::persistent(
        fixture.store.clone(),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    let mut prepared = machine
        .prepare_session_materialization(fixture.session_id.clone())
        .await
        .unwrap();
    let recovered = machine
        .sessions
        .read()
        .await
        .get(&fixture.session_id)
        .unwrap()
        .driver
        .clone();
    let mutation = machine
        .lock_current_session_driver_gate(&fixture.session_id, &recovered)
        .await
        .unwrap();
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(
        &recovered,
        &mut NoExecution,
    )
    .await
    .unwrap();
    drop(mutation);
    fixture.assert_finalized(&input_id).await;
    prepared.rollback_now().await.unwrap();
}

#[tokio::test]
async fn checkpoint_result_authority_cannot_be_rebound_to_another_batch_witness() {
    let fixture = Fixture::new().await;
    let input_id = old_input_id();
    fixture.checkpoint(input_id.clone()).await;
    let before = fixture.raw_row(&input_id);
    for defect in [
        "run",
        "owner",
        "recipient",
        "candidate_digest",
        "recipient_digest",
        "checkpoint",
        "live_boundary",
    ] {
        let entry = fixture.driver.lock().await;
        let mut witness = entry
            .input_terminal_completion_authorization_witness(std::slice::from_ref(&input_id))
            .unwrap();
        let authority = machine_resolve_runtime_completion_result_for_batch(
            &entry,
            &witness,
            Some(&fixture.run_id),
            mm::RuntimeCompletionTerminalObservation::NoResult,
            mm::RuntimeCompletionFinalizationObservation::Succeeded,
        )
        .unwrap();
        match defect {
            "run" => {
                witness.batch_key = crate::input_state::InputTerminalCompletionBatchKey::Run {
                    run_id: RunId::new(),
                }
            }
            "owner" => witness.owner_input_id = InputId::new(),
            "recipient" => {
                witness
                    .recipients
                    .insert(InputId::new(), InputTerminalOutcome::Consumed);
            }
            "candidate_digest" => witness.candidate_digest = "sha256:other".to_string(),
            "recipient_digest" => witness.completion_input_ids_digest = "sha256:other".to_string(),
            "checkpoint" => witness.requires_session_checkpoint = true,
            "live_boundary" => {
                witness.completion_boundary = Some(mm::RecoveredRunApplyBoundary::RunStart);
            }
            _ => unreachable!(),
        }
        assert!(
            crate::completion::authorize_runtime_terminal_bundle(
                &[],
                Some(&meerkat_core::lifecycle::core_executor::CoreApplyTerminal::NoPendingBoundary),
                authority,
                witness,
                None,
                None,
            )
            .is_err(),
            "{defect}"
        );
        drop(entry);
        assert_eq!(before, fixture.raw_row(&input_id), "{defect}");
    }
}

#[tokio::test]
async fn checkpoint_exact_receipt_cas_conflict_preserves_concurrent_row_and_newer_run() {
    let fixture = Fixture::new().await;
    let input_id = old_input_id();
    fixture.checkpoint(input_id.clone()).await;
    let newer = newer_run_owns_correlation(&fixture).await;
    let mut entry = fixture.driver.lock().await;
    let witness = entry
        .input_terminal_completion_authorization_witness(std::slice::from_ref(&input_id))
        .unwrap();
    let authority = machine_resolve_runtime_completion_result_for_batch(
        &entry,
        &witness,
        Some(&fixture.run_id),
        mm::RuntimeCompletionTerminalObservation::NoResult,
        mm::RuntimeCompletionFinalizationObservation::Succeeded,
    )
    .unwrap();
    let bundle = crate::completion::authorize_runtime_terminal_bundle(
        &[],
        Some(&meerkat_core::lifecycle::core_executor::CoreApplyTerminal::NoPendingBoundary),
        authority,
        witness,
        None,
        None,
    )
    .unwrap();
    let mut changed = fixture
        .store
        .load_input_state(&fixture.runtime_id(), &input_id)
        .await
        .unwrap()
        .unwrap();
    changed.state.updated_at += chrono::Duration::seconds(1);
    let concurrent = serde_json::to_vec(&changed).unwrap();
    fixture.replace_fixture_row(&input_id, &concurrent);
    let error = entry
        .finalize_input_terminal_completion_batch(bundle.terminal_completion())
        .await
        .expect_err("exact receipt CAS must reject a concurrently changed source row");
    assert!(matches!(error, RuntimeDriverError::StaleAuthority { .. }));
    assert_eq!(concurrent, fixture.raw_row(&input_id));
    let shared = entry.shared_dsl_authority();
    assert_run_unchanged(&newer, shared.lock().unwrap().state());
}
