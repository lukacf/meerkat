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
        Self::with_initial_input(true).await
    }

    async fn with_initial_input(include_main: bool) -> Self {
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
        let main_id = if include_main {
            let main = Input::Prompt(PromptInput::new("run in progress", None));
            let main_id = main.id().clone();
            driver.accept_input(main).await.unwrap();
            Some(main_id)
        } else {
            None
        };
        let run_id = RunId::from_uuid(
            uuid::Uuid::parse_str("01a0826a-f847-7ef2-8753-400d56761ce1").unwrap(),
        );
        driver.contract_begin_run_authority(run_id.clone()).unwrap();
        if let Some(main_id) = main_id {
            driver
                .machine_realize_authorized_stage_batch(test_authorized_stage_for_run(
                    vec![main_id],
                    run_id.clone(),
                ))
                .unwrap();
        }
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
        let input = checkpoint_peer_input(input_id.clone());
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

    async fn checkpoint_batch(&self, input_ids: &[InputId]) {
        self.checkpoint_batch_with_terminal(input_ids, None).await;
    }

    async fn checkpoint_batch_with_terminal(
        &self,
        input_ids: &[InputId],
        terminal: Option<&meerkat_core::lifecycle::core_executor::CoreApplyTerminal>,
    ) {
        let mut entry = self.driver.lock().await;
        for input_id in input_ids {
            let input = checkpoint_peer_input(input_id.clone());
            let admission = entry
                .resolve_admission_with_active_turn_boundary(&input, true)
                .unwrap();
            entry.accept_resolved_input(input, admission).await.unwrap();
        }
        let stage = machine_authorize_stage_for_run(
            &entry,
            &self.run_id,
            input_ids,
            RuntimeLoopBatchSource::Steer,
        )
        .expect("the generated same-boundary batch must authorize both inputs");
        if let DriverEntry::Persistent(driver) = &mut *entry {
            driver
                .machine_realize_authorized_stage_batch(stage)
                .unwrap();
        }
        drop(entry);
        commit_runtime_loop_run(
            &self.driver,
            self.run_id.clone(),
            input_ids.to_vec(),
            meerkat_core::lifecycle::RunBoundaryReceiptDraft {
                run_id: self.run_id.clone(),
                boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                contributing_input_ids: input_ids.to_vec(),
                conversation_digest: None,
                message_count: 0,
            },
            None,
            Vec::new(),
            terminal,
        )
        .await
        .expect("the actual runtime batch boundary must commit");
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
        driver
            .recover_inputs_after_runtime_authority(None, None)
            .await?;
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

fn checkpoint_peer_input(input_id: InputId) -> Input {
    let mut header = PromptInput::new("", None).header;
    header.id = input_id;
    header.timestamp = chrono::DateTime::parse_from_rfc3339("2026-09-08T19:09:09.690958Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    header.source = InputOrigin::Peer {
        peer_id: "checkpoint-peer".into(),
        display_identity: None,
        runtime_id: None,
    };
    Input::Peer(PeerInput {
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
        in_reply_to: None,
    })
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
async fn checkpoint_apply_with_run_result_uses_run_authority_live_and_after_restart() {
    use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;

    for cold in [false, true] {
        let fixture = Fixture::with_initial_input(false).await;
        let input_id = old_input_id();
        let terminal = CoreApplyTerminal::RunResult(Box::new(meerkat_core::types::RunResult {
            reasoning_effort: None,
            text: "queued Steer produced a full run result".to_string(),
            session_id: fixture.session_id.clone(),
            usage: Default::default(),
            turns: 1,
            tool_calls: 0,
            terminal_cause_kind: None,
            structured_output: None,
            extraction_error: None,
            schema_warnings: None,
            skill_diagnostics: None,
            run_usage: None,
            request_usage: Vec::new(),
        }));
        fixture
            .checkpoint_batch_with_terminal(std::slice::from_ref(&input_id), Some(&terminal))
            .await;
        let driver = if cold {
            fixture.fresh_driver_from_durable().await.unwrap()
        } else {
            fixture.driver.clone()
        };
        crate::runtime_loop::test_drain_recovered_input_terminal_completions(
            &driver,
            &mut NoExecution,
        )
        .await
        .expect("the actual terminal, not the apply boundary, selects run-result authority");
        let row = fixture
            .store
            .load_input_state(&fixture.runtime_id(), &input_id)
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
        let Some(crate::completion::CompletionOutcome::Completed(result)) = completion.outcome
        else {
            panic!("full run completion must retain its RunResult");
        };
        assert_eq!(result.session_id, fixture.session_id);
        assert_eq!(result.text, "queued Steer produced a full run result");
        let finalized = fixture.raw_row(&input_id);
        crate::runtime_loop::test_drain_recovered_input_terminal_completions(
            &driver,
            &mut NoExecution,
        )
        .await
        .unwrap();
        assert_eq!(finalized, fixture.raw_row(&input_id));
        assert_eq!(
            fixture
                .store
                .load_session_snapshot(&fixture.runtime_id())
                .await
                .unwrap()
                .unwrap(),
            fixture.transcript,
        );
        assert_eq!(
            fixture
                .store
                .load_whole_blob_store_authority(&fixture.runtime_id())
                .await
                .unwrap(),
            Some(fixture.authority),
        );
    }
}

/// A run result an earlier release committed but never finalized (its turn
/// stopped before the completion receipt settled) is finalized from the
/// durable row at the next cold start, so the session can accept work again.
/// The row still carries the earlier release's `stored_input_state_version`
/// stamp, which makes its bytes differ from this binary's image of the same
/// typed row; the exact receipt CAS must still recognize it.
#[tokio::test]
async fn earlier_release_pending_run_result_finalizes_after_cold_restart() {
    use meerkat_core::generated::session_persistence_version_authority as version_authority;
    use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;

    let fixture = Fixture::with_initial_input(false).await;
    let input_id = old_input_id();
    let terminal = CoreApplyTerminal::RunResult(Box::new(meerkat_core::types::RunResult {
        reasoning_effort: None,
        text: "an earlier release committed this result".to_string(),
        session_id: fixture.session_id.clone(),
        usage: Default::default(),
        turns: 1,
        tool_calls: 0,
        terminal_cause_kind: None,
        structured_output: None,
        extraction_error: None,
        schema_warnings: None,
        skill_diagnostics: None,
        run_usage: None,
        request_usage: Vec::new(),
    }));
    fixture
        .checkpoint_batch_with_terminal(std::slice::from_ref(&input_id), Some(&terminal))
        .await;
    let current = String::from_utf8(fixture.raw_row(&input_id)).unwrap();
    let current_stamp = format!(
        "\"stored_input_state_version\":{}",
        version_authority::stored_input_state_version_for_row(false)
    );
    let earlier = current.replacen(&current_stamp, "\"stored_input_state_version\":6", 1);
    assert_ne!(earlier, current, "the fixture must carry the earlier stamp");
    fixture.replace_fixture_row(&input_id, earlier.as_bytes());

    let driver = fixture.fresh_driver_from_durable().await.unwrap();
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(&driver, &mut NoExecution)
        .await
        .expect("the earlier release's pending receipt finalizes from its durable row");
    let row = fixture
        .store
        .load_input_state(&fixture.runtime_id(), &input_id)
        .await
        .unwrap()
        .unwrap();
    let completion = row.state.terminal_completion.unwrap();
    assert!(matches!(
        completion.phase,
        InputTerminalCompletionPhase::Finalized { .. }
    ));
    let Some(crate::completion::CompletionOutcome::Completed(result)) = completion.outcome else {
        panic!("the committed RunResult must survive finalization");
    };
    assert_eq!(result.text, "an earlier release committed this result");
    let finalized = fixture.raw_row(&input_id);
    assert!(
        std::str::from_utf8(&finalized)
            .unwrap()
            .contains(&current_stamp),
        "finalization writes the current encoding"
    );
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(&driver, &mut NoExecution)
        .await
        .unwrap();
    assert_eq!(finalized, fixture.raw_row(&input_id));
}

/// Stage `input_id` for the fixture run, end the turn with the generated
/// budget terminal, and commit it through the real runtime-loop commit
/// carrying `terminal`.
async fn commit_budget_terminal_batch(
    fixture: &Fixture,
    input_id: &InputId,
    terminal: &meerkat_core::lifecycle::core_executor::CoreApplyTerminal,
) {
    let mut entry = fixture.driver.lock().await;
    let input = checkpoint_peer_input(input_id.clone());
    let admission = entry
        .resolve_admission_with_active_turn_boundary(&input, true)
        .unwrap();
    entry.accept_resolved_input(input, admission).await.unwrap();
    let stage = machine_authorize_stage_for_run(
        &entry,
        &fixture.run_id,
        std::slice::from_ref(input_id),
        RuntimeLoopBatchSource::Steer,
    )
    .expect("the batch authorizes for the run");
    if let DriverEntry::Persistent(driver) = &mut *entry {
        driver
            .machine_realize_authorized_stage_batch(stage)
            .unwrap();
    }
    {
        let shared = entry.shared_dsl_authority();
        let mut machine = shared.lock().unwrap();
        mm::MeerkatMachineMutator::apply(
            &mut *machine,
            mm::MeerkatMachineInput::BudgetExhausted {
                run_id: mm::RunId::from_domain(&fixture.run_id),
            },
        )
        .expect("the generated budget terminal applies to the run");
    }
    if let DriverEntry::Persistent(driver) = &mut *entry {
        driver
            .inner_mut()
            .sync_control_projection_from_dsl_authority();
    }
    drop(entry);
    commit_runtime_loop_run(
        &fixture.driver,
        fixture.run_id.clone(),
        vec![input_id.clone()],
        meerkat_core::lifecycle::RunBoundaryReceiptDraft {
            run_id: fixture.run_id.clone(),
            boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
            contributing_input_ids: vec![input_id.clone()],
            conversation_digest: None,
            message_count: 0,
        },
        None,
        Vec::new(),
        Some(terminal),
    )
    .await
    .expect("a coherent budget terminal commits through the real run commit");
}

fn budget_run_result(session_id: &SessionId) -> meerkat_core::types::RunResult {
    meerkat_core::types::RunResult {
        reasoning_effort: None,
        text: String::new(),
        session_id: session_id.clone(),
        usage: Default::default(),
        turns: 1,
        tool_calls: 0,
        terminal_cause_kind: Some(meerkat_core::TurnTerminalCauseKind::BudgetExhausted),
        structured_output: None,
        extraction_error: None,
        schema_warnings: None,
        skill_diagnostics: None,
        run_usage: None,
        request_usage: Vec::new(),
    }
}

#[tokio::test]
async fn budget_run_result_recovers_after_a_completion_receipt_write_fault() {
    use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;

    for cold in [false, true] {
        let fixture = Fixture::with_initial_input(false).await;
        let input_id = old_input_id();
        let expected = budget_run_result(&fixture.session_id);
        let terminal = CoreApplyTerminal::RunResult(Box::new(expected.clone()));
        commit_budget_terminal_batch(&fixture, &input_id, &terminal).await;

        // The committed boundary: the exact run, input and budget candidate.
        let receipts = fixture
            .store
            .load_committed_boundary_receipts(&fixture.runtime_id(), &fixture.run_id)
            .await
            .unwrap();
        assert_eq!(receipts.len(), 1, "one boundary receipt for the run");
        assert_eq!(receipts[0].run_id, fixture.run_id);
        assert_eq!(receipts[0].contributing_input_ids, vec![input_id.clone()]);
        let pending = fixture
            .store
            .load_input_state(&fixture.runtime_id(), &input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pending.seed.phase, InputLifecycleState::Consumed);
        let completion = pending.state.terminal_completion.as_ref().unwrap();
        assert_eq!(completion.owner_input_id, input_id);
        assert_eq!(completion.batch_key.run_id(), Some(&fixture.run_id));
        assert!(matches!(
            completion.phase,
            InputTerminalCompletionPhase::Pending
        ));
        let Some(InteractionTerminalCandidate::RunResult { result }) =
            completion.candidate.as_ref()
        else {
            panic!("the budget run retains its typed result as the candidate");
        };
        assert_eq!(
            serde_json::to_value(result.as_ref()).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        let committed = fixture.raw_row(&input_id);

        // A transient fault refuses the completion-receipt write.
        rusqlite::Connection::open(fixture.store.path())
            .unwrap()
            .execute_batch(&format!(
                "CREATE TRIGGER fail_budget_completion_receipt BEFORE UPDATE ON runtime_input_states
             WHEN NEW.runtime_id = '{}' AND NEW.input_id = '{}'
             BEGIN SELECT RAISE(ABORT, 'synthetic completion receipt fault'); END;",
                fixture.runtime_id(),
                input_id,
            ))
            .unwrap();
        let driver = if cold {
            fixture.fresh_driver_from_durable().await.unwrap()
        } else {
            fixture.driver.clone()
        };
        crate::runtime_loop::test_drain_recovered_input_terminal_completions(
            &driver,
            &mut NoExecution,
        )
        .await
        .expect_err("the faulted completion-receipt write is refused");
        assert_eq!(
            committed,
            fixture.raw_row(&input_id),
            "the committed candidate survives the fault unchanged"
        );

        // A real non-directed recovery finalizes the same budget result.
        rusqlite::Connection::open(fixture.store.path())
            .unwrap()
            .execute_batch("DROP TRIGGER fail_budget_completion_receipt;")
            .unwrap();
        let driver = if cold {
            fixture.fresh_driver_from_durable().await.unwrap()
        } else {
            driver
        };
        crate::runtime_loop::test_drain_recovered_input_terminal_completions(
            &driver,
            &mut NoExecution,
        )
        .await
        .expect("non-directed recovery finalizes the retained budget result");
        let row = fixture
            .store
            .load_input_state(&fixture.runtime_id(), &input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            row.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        assert_eq!(
            row.state
                .history
                .iter()
                .filter(|event| event.to == InputLifecycleState::Consumed)
                .count(),
            1,
            "the input is consumed exactly once"
        );
        let completion = row.state.terminal_completion.unwrap();
        assert_eq!(completion.batch_key.run_id(), Some(&fixture.run_id));
        assert!(matches!(
            completion.phase,
            InputTerminalCompletionPhase::Finalized { .. }
        ));
        let Some(crate::completion::CompletionOutcome::Completed(result)) = completion.outcome
        else {
            panic!("the budget run completes with its typed result");
        };
        assert_eq!(
            serde_json::to_value(result.as_ref()).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        assert_eq!(
            result.terminal_cause_kind,
            Some(meerkat_core::TurnTerminalCauseKind::BudgetExhausted)
        );
        assert_eq!(
            fixture
                .store
                .load_committed_boundary_receipts(&fixture.runtime_id(), &fixture.run_id)
                .await
                .unwrap(),
            receipts,
            "the boundary receipt keeps its identity and sequence"
        );

        // A repeat drain is stable.
        let finalized = fixture.raw_row(&input_id);
        crate::runtime_loop::test_drain_recovered_input_terminal_completions(
            &driver,
            &mut NoExecution,
        )
        .await
        .unwrap();
        assert_eq!(finalized, fixture.raw_row(&input_id));
    }
}

#[tokio::test]
async fn multi_recipient_checkpoint_finalizes_exact_cohort_once_live_and_after_restart() {
    for cold in [false, true] {
        let fixture = Fixture::with_initial_input(false).await;
        let inputs = [old_input_id(), InputId::new()];
        fixture.checkpoint_batch(&inputs).await;
        let owner = fixture.pending(&inputs[0]).await;
        assert_eq!(
            owner
                .state
                .terminal_completion
                .as_ref()
                .unwrap()
                .completion_input_ids
                .as_deref(),
            Some(inputs.as_slice()),
        );
        let driver = if cold {
            fixture.fresh_driver_from_durable().await.unwrap()
        } else {
            fixture.driver.clone()
        };
        crate::runtime_loop::test_drain_recovered_input_terminal_completions(
            &driver,
            &mut NoExecution,
        )
        .await
        .unwrap();
        fixture.assert_finalized(&inputs[0]).await;
        let second = fixture
            .store
            .load_input_state(&fixture.runtime_id(), &inputs[1])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            second.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        let completion = second.state.terminal_completion.unwrap();
        assert_eq!(completion.owner_input_id, inputs[0]);
        assert!(matches!(
            completion.phase,
            InputTerminalCompletionPhase::Finalized { .. }
        ));
        assert!(
            completion.outcome.is_none(),
            "only the exact owner stores the shared outcome"
        );
        let receipts = inputs
            .iter()
            .map(|id| fixture.raw_row(id))
            .collect::<Vec<_>>();
        crate::runtime_loop::test_drain_recovered_input_terminal_completions(
            &driver,
            &mut NoExecution,
        )
        .await
        .unwrap();
        assert_eq!(
            receipts,
            inputs
                .iter()
                .map(|id| fixture.raw_row(id))
                .collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn multi_recipient_checkpoint_refuses_a_nonowner_with_wrong_run_or_terminal() {
    for wrong_run in [true, false] {
        let fixture = Fixture::with_initial_input(false).await;
        let inputs = [old_input_id(), InputId::new()];
        fixture.checkpoint_batch(&inputs).await;
        let owner_before = fixture.raw_row(&inputs[0]);
        let mut other = fixture
            .store
            .load_input_state(&fixture.runtime_id(), &inputs[1])
            .await
            .unwrap()
            .unwrap();
        if wrong_run {
            other.seed.last_run_id = Some(RunId::new());
        } else {
            other.seed.phase = InputLifecycleState::Abandoned;
            other.seed.terminal_outcome = Some(InputTerminalOutcome::Abandoned {
                reason: crate::input_state::InputAbandonReason::Cancelled,
            });
        }
        let bytes = serde_json::to_vec(&other).unwrap();
        fixture.replace_fixture_row(&inputs[1], &bytes);
        if let Ok(fresh) = fixture.fresh_driver_from_durable().await {
            assert!(
                crate::runtime_loop::test_drain_recovered_input_terminal_completions(
                    &fresh,
                    &mut NoExecution,
                )
                .await
                .is_err()
            );
        }
        assert_eq!(owner_before, fixture.raw_row(&inputs[0]));
        assert_eq!(bytes, fixture.raw_row(&inputs[1]));
    }
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
async fn downstream_consumed_checkpoint_recovers_from_attached_without_replacing_later_correlation()
{
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
                residency_owner: mm::SessionResidencyOwner::Session,
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
    let machine = Arc::new(
        super::super::MeerkatMachine::persistent(
            fixture.store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("persistent machine"),
    );
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
    for defect in ["run", "owner", "recipients", "candidate", "digest"] {
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
            "digest" => completion.candidate_digest = "sha256:wrong".to_string(),
            _ => unreachable!(),
        }
        let bytes = serde_json::to_vec(&row).unwrap();
        fixture.replace_fixture_row(&input_id, &bytes);
        let fresh = fixture.fresh_driver_from_durable().await;
        if let Ok(fresh) = fresh {
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
    let machine = Arc::new(
        super::super::MeerkatMachine::persistent(
            fixture.store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("persistent machine"),
    );
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

// ---------------------------------------------------------------------------
// A consumed input whose completion is still pending when its host ends is the
// terminal carrier of a committed run. The next start recovers it and its
// first drain finalizes it as a live host would have, whatever durability the
// input was admitted with. Durability decides only whether unfinished work
// survives a restart.
// ---------------------------------------------------------------------------

use crate::input::InputDurability;

fn carrier_input(input_id: InputId, durability: InputDurability) -> Input {
    let mut input = checkpoint_peer_input(input_id);
    if let Input::Peer(peer) = &mut input {
        peer.header.durability = durability;
    }
    input
}

fn carrier_run_result(session_id: &SessionId, text: &str) -> meerkat_core::types::RunResult {
    meerkat_core::types::RunResult {
        reasoning_effort: None,
        text: text.to_string(),
        session_id: session_id.clone(),
        usage: Default::default(),
        turns: 1,
        tool_calls: 0,
        terminal_cause_kind: None,
        structured_output: None,
        extraction_error: None,
        schema_warnings: None,
        skill_diagnostics: None,
        run_usage: None,
        request_usage: Vec::new(),
    }
}

/// Records every committed session snapshot the startup drain hands it and
/// refuses everything else a recovered completion must not ask for.
#[derive(Default)]
struct SessionCheckpointRecorder {
    snapshots: Vec<Arc<Vec<u8>>>,
}

#[async_trait::async_trait]
impl CoreExecutor for SessionCheckpointRecorder {
    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        panic!("recovering a consumed input must not cancel a run")
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        panic!("recovering a consumed input must not stop a run")
    }

    async fn apply(
        &mut self,
        _run_id: RunId,
        _primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        panic!("recovering a consumed input must not execute a turn")
    }

    async fn checkpoint_committed_session_snapshot(
        &mut self,
        snapshot: Arc<Vec<u8>>,
    ) -> Result<(), CoreExecutorError> {
        self.snapshots.push(snapshot);
        Ok(())
    }

    async fn publish_interaction_terminals(
        &mut self,
        _events: &[meerkat_core::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        panic!("a non-directed recovered completion must not invent a publication")
    }
}

impl Fixture {
    /// Admit `inputs`, each with its durability, and consume them in one
    /// boundary of the fixture run through the real runtime-loop commit. With
    /// `session`, the commit carries that sealed session, so the completion
    /// batch requires a session checkpoint.
    async fn consume_batch(
        &self,
        inputs: &[(InputId, InputDurability)],
        session: Option<meerkat_core::Session>,
        terminal: Option<&meerkat_core::lifecycle::core_executor::CoreApplyTerminal>,
    ) {
        let input_ids = inputs
            .iter()
            .map(|(input_id, _)| input_id.clone())
            .collect::<Vec<_>>();
        let mut entry = self.driver.lock().await;
        for (input_id, durability) in inputs {
            let input = carrier_input(input_id.clone(), *durability);
            let admission = entry
                .resolve_admission_with_active_turn_boundary(&input, true)
                .unwrap();
            entry.accept_resolved_input(input, admission).await.unwrap();
        }
        let stage = machine_authorize_stage_for_run(
            &entry,
            &self.run_id,
            &input_ids,
            RuntimeLoopBatchSource::Steer,
        )
        .expect("the generated same-boundary batch must authorize every input");
        if let DriverEntry::Persistent(driver) = &mut *entry {
            driver
                .machine_realize_authorized_stage_batch(stage)
                .unwrap();
        }
        drop(entry);
        let committed = session.map(|session| {
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(Arc::new(session))
                .expect("seal the committed session")
        });
        let (conversation_digest, message_count) = match committed.as_ref() {
            Some(committed) => {
                let (_, message_count, digest) = sealed_session_boundary_facts(committed)
                    .expect("the sealed session states its boundary facts");
                (Some(digest), message_count)
            }
            None => (None, 0),
        };
        commit_runtime_loop_run(
            &self.driver,
            self.run_id.clone(),
            input_ids.clone(),
            meerkat_core::lifecycle::RunBoundaryReceiptDraft {
                run_id: self.run_id.clone(),
                boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                contributing_input_ids: input_ids,
                conversation_digest,
                message_count,
            },
            committed,
            Vec::new(),
            terminal,
        )
        .await
        .expect("the actual runtime batch boundary must commit");
    }

    fn pending_terminal_owner_rows(&self, input_id: &InputId) -> usize {
        rusqlite::Connection::open(self.store.path())
            .unwrap()
            .query_row(
                "SELECT count(*) FROM runtime_pending_terminal_owners WHERE runtime_id=?1 AND owner_input_id=?2",
                rusqlite::params![self.runtime_id().to_string(), input_id.to_string()],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn set_pending_terminal_owner_row(&self, input_id: &InputId, present: bool) {
        let sql = if present {
            "INSERT INTO runtime_pending_terminal_owners (runtime_id, owner_input_id) VALUES (?1, ?2)"
        } else {
            "DELETE FROM runtime_pending_terminal_owners WHERE runtime_id=?1 AND owner_input_id=?2"
        };
        let changed = rusqlite::Connection::open(self.store.path())
            .unwrap()
            .execute(
                sql,
                rusqlite::params![self.runtime_id().to_string(), input_id.to_string()],
            )
            .unwrap();
        assert_eq!(changed, 1, "exactly one pending terminal owner row changes");
    }

    async fn stored_durability(&self, input_id: &InputId) -> Option<InputDurability> {
        self.store
            .load_input_state(&self.runtime_id(), input_id)
            .await
            .unwrap()
            .expect("the consumed row stays stored")
            .state
            .durability
    }

    /// A machine over the fixture's store, as the next host opens it.
    fn next_host(&self) -> Arc<super::super::MeerkatMachine> {
        Arc::new(
            super::super::MeerkatMachine::persistent(
                self.store.clone(),
                Arc::new(meerkat_store::MemoryBlobStore::new()),
            )
            .expect("persistent machine"),
        )
    }
}

/// A host ends after a run consumed one input and before its completion was
/// finalized. The next start recovers the carrier, and its first drain
/// finalizes the completion with the typed outcome and clears the pending
/// terminal index.
async fn the_first_drain_after_a_restart_finalizes_a_terminal_carrier(durability: InputDurability) {
    let fixture = Fixture::with_initial_input(false).await;
    let input_id = old_input_id();
    fixture
        .consume_batch(&[(input_id.clone(), durability)], None, None)
        .await;
    let pending = fixture.pending(&input_id).await;
    assert_eq!(pending.state.durability, Some(durability));
    assert_eq!(fixture.pending_terminal_owner_rows(&input_id), 1);

    let driver = fixture
        .fresh_driver_from_durable()
        .await
        .expect("the next start recovers the store");
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(&driver, &mut NoExecution)
        .await
        .expect("the startup drain finalizes the carried completion");
    fixture.assert_finalized(&input_id).await;
    assert_eq!(fixture.pending_terminal_owner_rows(&input_id), 0);
    assert_eq!(
        fixture.stored_durability(&input_id).await,
        Some(durability),
        "the finalized row stays stored as the input was admitted"
    );
}

#[tokio::test]
async fn a_durable_terminal_carrier_is_finalized_by_the_first_drain_after_a_restart() {
    the_first_drain_after_a_restart_finalizes_a_terminal_carrier(InputDurability::Durable).await;
}

#[tokio::test]
async fn an_ephemeral_terminal_carrier_is_finalized_by_the_first_drain_after_a_restart() {
    the_first_drain_after_a_restart_finalizes_a_terminal_carrier(InputDurability::Ephemeral).await;
}

/// Consume `input_id` in a run whose commit carries its session, so its
/// completion batch requires a session checkpoint, and answer the snapshot
/// that commit stored. The completion is left pending.
async fn consume_with_committed_session(
    fixture: &Fixture,
    input_id: &InputId,
    durability: InputDurability,
) -> Arc<Vec<u8>> {
    use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;

    let mut session = meerkat_core::Session::with_id(fixture.session_id.clone());
    session.push(Message::User(UserMessage::text("retained history")));
    session.push(Message::User(UserMessage::text("the committed turn")));
    let terminal = CoreApplyTerminal::RunResult(Box::new(carrier_run_result(
        &fixture.session_id,
        COMMITTED_RUN_TEXT,
    )));
    fixture
        .consume_batch(
            &[(input_id.clone(), durability)],
            Some(session),
            Some(&terminal),
        )
        .await;
    let pending = fixture
        .store
        .load_input_state(&fixture.runtime_id(), input_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(pending.state.durability, Some(durability));
    let completion = pending.state.terminal_completion.as_ref().unwrap();
    assert!(completion.requires_session_checkpoint);
    assert!(matches!(
        completion.phase,
        InputTerminalCompletionPhase::Pending
    ));
    assert_eq!(fixture.pending_terminal_owner_rows(input_id), 1);
    let committed_snapshot = fixture
        .store
        .load_session_snapshot(&fixture.runtime_id())
        .await
        .unwrap()
        .expect("the run's commit stored its session");
    assert_ne!(
        committed_snapshot, fixture.transcript,
        "the committed session is the run's, not the earlier history"
    );
    committed_snapshot
}

const COMMITTED_RUN_TEXT: &str = "the run committed this result before its host ended";

/// The completion of `input_id` is finalized with the committed run's result
/// and the store names no pending terminal owner for it.
async fn assert_committed_run_finalized(fixture: &Fixture, input_id: &InputId) {
    let row = fixture
        .store
        .load_input_state(&fixture.runtime_id(), input_id)
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
    let Some(crate::completion::CompletionOutcome::Completed(result)) = completion.outcome else {
        panic!("the committed run result must survive finalization");
    };
    assert_eq!(result.text, COMMITTED_RUN_TEXT);
    assert_eq!(fixture.pending_terminal_owner_rows(input_id), 0);
}

/// The same host end, for a run whose commit carried its session: the first
/// drain after the restart takes the session checkpoint the batch requires,
/// once and with the committed snapshot, and then finalizes the completion
/// with the run's result.
async fn the_first_drain_after_a_restart_checkpoints_the_session_of_a_terminal_carrier(
    durability: InputDurability,
) {
    let fixture = Fixture::with_initial_input(false).await;
    let input_id = old_input_id();
    let committed_snapshot = consume_with_committed_session(&fixture, &input_id, durability).await;

    let driver = fixture
        .fresh_driver_from_durable()
        .await
        .expect("the next start recovers the store");
    let mut executor = SessionCheckpointRecorder::default();
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(&driver, &mut executor)
        .await
        .expect("the startup drain checkpoints the session and finalizes the completion");
    assert_eq!(
        executor.snapshots,
        vec![committed_snapshot],
        "the drain takes the session checkpoint once, with the committed snapshot"
    );
    assert_committed_run_finalized(&fixture, &input_id).await;
}

#[tokio::test]
async fn the_first_drain_after_a_restart_checkpoints_the_session_of_a_durable_terminal_carrier() {
    the_first_drain_after_a_restart_checkpoints_the_session_of_a_terminal_carrier(
        InputDurability::Durable,
    )
    .await;
}

#[tokio::test]
async fn the_first_drain_after_a_restart_checkpoints_the_session_of_an_ephemeral_terminal_carrier()
{
    the_first_drain_after_a_restart_checkpoints_the_session_of_a_terminal_carrier(
        InputDurability::Ephemeral,
    )
    .await;
}

/// A run whose result carries a JSON-null structured output commits, and its
/// host ends before the completion is finalized. The next start recovers the
/// store, and its first drain takes the session checkpoint and finalizes the
/// completion with the run's result, the null structured output included.
/// Such a row used to fail its digest check on every read (the result was
/// written with an explicit null and read back without it), so the next
/// start could not read its pending terminal owner and the session could not
/// be registered again.
#[tokio::test]
async fn a_terminal_carrier_with_a_null_structured_output_is_recovered_at_the_next_start() {
    use meerkat_core::lifecycle::core_executor::CoreApplyTerminal;

    let fixture = Fixture::with_initial_input(false).await;
    let input_id = old_input_id();
    let mut session = meerkat_core::Session::with_id(fixture.session_id.clone());
    session.push(Message::User(UserMessage::text("retained history")));
    session.push(Message::User(UserMessage::text("the committed turn")));
    let mut result = carrier_run_result(&fixture.session_id, COMMITTED_RUN_TEXT);
    result.structured_output = Some(serde_json::Value::Null);
    let terminal = CoreApplyTerminal::RunResult(Box::new(result));
    fixture
        .consume_batch(
            &[(input_id.clone(), InputDurability::Durable)],
            Some(session),
            Some(&terminal),
        )
        .await;
    assert_eq!(fixture.pending_terminal_owner_rows(&input_id), 1);
    let committed_snapshot = fixture
        .store
        .load_session_snapshot(&fixture.runtime_id())
        .await
        .unwrap()
        .expect("the run's commit stored its session");

    let driver = fixture
        .fresh_driver_from_durable()
        .await
        .expect("the next start recovers the store");
    let mut executor = SessionCheckpointRecorder::default();
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(&driver, &mut executor)
        .await
        .expect("the startup drain checkpoints the session and finalizes the completion");
    assert_eq!(executor.snapshots, vec![committed_snapshot]);

    let row = fixture
        .store
        .load_input_state(&fixture.runtime_id(), &input_id)
        .await
        .expect("the finalized row reads")
        .expect("the finalized row is stored");
    let completion = row.state.terminal_completion.unwrap();
    assert!(matches!(
        completion.phase,
        InputTerminalCompletionPhase::Finalized { .. }
    ));
    let Some(crate::completion::CompletionOutcome::Completed(result)) = completion.outcome else {
        panic!("the committed run result must survive finalization");
    };
    assert_eq!(result.text, COMMITTED_RUN_TEXT);
    assert_eq!(result.structured_output, Some(serde_json::Value::Null));
    assert_eq!(fixture.pending_terminal_owner_rows(&input_id), 0);
}

/// A stored-only publisher for a drain that must publish nothing.
struct NoPublication;

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutorPublicationHandle for NoPublication {
    async fn publish_interaction_terminals(
        &self,
        _events: &[meerkat_core::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        CoreExecutorError,
    > {
        panic!("a non-directed recovered completion must not invent a publication")
    }
}

/// A drain without an executor cannot take a session checkpoint. It refuses
/// a recovered carrier whose batch requires one, typed, and leaves the row
/// and the pending terminal index as they were; the executor's drain then
/// takes the checkpoint and finalizes the completion. An ephemeral carrier
/// is treated exactly as a durable one.
async fn a_drain_without_an_executor_leaves_a_checkpoint_owing_terminal_carrier_as_it_was(
    durability: InputDurability,
) {
    let fixture = Fixture::with_initial_input(false).await;
    let input_id = old_input_id();
    let committed_snapshot = consume_with_committed_session(&fixture, &input_id, durability).await;

    let driver = fixture
        .fresh_driver_from_durable()
        .await
        .expect("the next start recovers the store");
    let pending = fixture.raw_row(&input_id);
    let hooks = Arc::new(meerkat_core::PostCommitHookDispatcher::new(
        fixture.session_id.clone(),
    ));
    let refused = crate::runtime_loop::drain_recovered_terminals_with_stored_only_publisher(
        &driver,
        &hooks,
        None,
        &NoPublication,
    )
    .await
    .expect_err("a drain without an executor cannot take the session checkpoint");
    assert!(
        matches!(
            &refused,
            RuntimeDriverError::ValidationFailed { reason }
                if reason == "a recovered terminal completion batch needs a session checkpoint"
        ),
        "unexpected refusal: {refused}"
    );
    assert_eq!(
        pending,
        fixture.raw_row(&input_id),
        "the refused drain leaves the carrier as it was"
    );
    assert_eq!(fixture.pending_terminal_owner_rows(&input_id), 1);

    let mut executor = SessionCheckpointRecorder::default();
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(&driver, &mut executor)
        .await
        .expect("the executor's drain checkpoints the session and finalizes the completion");
    assert_eq!(executor.snapshots, vec![committed_snapshot]);
    assert_committed_run_finalized(&fixture, &input_id).await;
}

#[tokio::test]
async fn a_drain_without_an_executor_leaves_a_checkpoint_owing_durable_terminal_carrier_as_it_was()
{
    a_drain_without_an_executor_leaves_a_checkpoint_owing_terminal_carrier_as_it_was(
        InputDurability::Durable,
    )
    .await;
}

#[tokio::test]
async fn a_drain_without_an_executor_leaves_a_checkpoint_owing_ephemeral_terminal_carrier_as_it_was()
 {
    a_drain_without_an_executor_leaves_a_checkpoint_owing_terminal_carrier_as_it_was(
        InputDurability::Ephemeral,
    )
    .await;
}

/// The next host registers the session and is then asked to unregister it,
/// with the carried completion still pending: the unregister succeeds.
async fn an_unregister_after_a_restart_succeeds_with_a_terminal_carrier(
    durability: InputDurability,
) {
    let fixture = Fixture::with_initial_input(false).await;
    let input_id = old_input_id();
    fixture
        .consume_batch(&[(input_id.clone(), durability)], None, None)
        .await;
    fixture.pending(&input_id).await;

    let machine = fixture.next_host();
    machine
        .register_session(fixture.session_id.clone())
        .await
        .expect("the next start registers the session");
    machine
        .unregister_session(&fixture.session_id)
        .await
        .expect("the registered session unregisters while its completion is still carried");
    assert!(!machine.contains_session(&fixture.session_id).await);
    // The unregister neither finalizes nor drops the carried completion: the
    // row is still consumed with its completion pending, and the store still
    // names it, for the next start's drain.
    fixture.pending(&input_id).await;
    assert_eq!(fixture.pending_terminal_owner_rows(&input_id), 1);
}

#[tokio::test]
async fn an_unregister_after_a_restart_succeeds_with_a_durable_terminal_carrier() {
    an_unregister_after_a_restart_succeeds_with_a_terminal_carrier(InputDurability::Durable).await;
}

#[tokio::test]
async fn an_unregister_after_a_restart_succeeds_with_an_ephemeral_terminal_carrier() {
    an_unregister_after_a_restart_succeeds_with_a_terminal_carrier(InputDurability::Ephemeral)
        .await;
}

/// A store in the state an earlier release leaves: a consumed ephemeral
/// input whose run committed its session and whose completion is pending,
/// durable inputs still queued beside it, a registration that left the
/// carrier out of its live driver, and that registration's release refused
/// with its progress stored. The first registration of this build recovers
/// the carrier, its first drain takes the session checkpoint and finalizes
/// the completion, the registration is released, and the queued inputs are
/// still queued.
#[tokio::test]
async fn a_store_holding_an_unrecovered_ephemeral_terminal_carrier_heals_at_the_next_start() {
    let fixture = Fixture::with_initial_input(false).await;
    let input_id = old_input_id();
    let queued = [InputId::new(), InputId::new()];
    {
        let mut entry = fixture.driver.lock().await;
        for queued_id in &queued {
            let input = carrier_input(queued_id.clone(), InputDurability::Durable);
            let admission = entry
                .resolve_admission_with_active_turn_boundary(&input, true)
                .unwrap();
            entry.accept_resolved_input(input, admission).await.unwrap();
        }
    }
    let committed_snapshot =
        consume_with_committed_session(&fixture, &input_id, InputDurability::Ephemeral).await;
    let committed_carrier = fixture.raw_row(&input_id);

    // The earlier release's host: its registration does not load the carrier
    // (the index row is hidden from it and put back, which leaves the store
    // and the live driver exactly as that registration left them), and the
    // release of that registration, which keeps what it recovered, is
    // refused.
    fixture.set_pending_terminal_owner_row(&input_id, false);
    let earlier = fixture.next_host();
    let mut earlier_registration = earlier
        .prepare_session_materialization(fixture.session_id.clone())
        .await
        .expect("the earlier host registers the session");
    fixture.set_pending_terminal_owner_row(&input_id, true);
    let refused = earlier_registration
        .rollback_now()
        .await
        .expect_err("a live driver that lacks an owner the store names refuses its release");
    assert!(
        refused
            .to_string()
            .contains("is absent from the live shell"),
        "unexpected refusal: {refused}"
    );
    drop(earlier_registration);
    drop(earlier);

    // What that host left in the store.
    let left = crate::store::load_machine_lifecycle(fixture.store.as_ref(), &fixture.runtime_id())
        .await
        .unwrap()
        .expect("the refused release left the session's lifecycle row");
    assert_eq!(left.runtime_state(), crate::RuntimeState::Stopped);
    assert!(
        left.unregister_progress()
            .is_some_and(|progress| progress.keeps_recovered_inputs()),
        "the refused release left its progress, keeping what it recovered: {:?}",
        left.unregister_progress()
    );
    assert_eq!(
        committed_carrier,
        fixture.raw_row(&input_id),
        "the carrier is as the run's commit left it"
    );
    assert_eq!(fixture.pending_terminal_owner_rows(&input_id), 1);

    // This build's host, on the same store.
    let machine = fixture.next_host();
    let mut registration = machine
        .prepare_session_materialization(fixture.session_id.clone())
        .await
        .expect("the next start registers the session");
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
    let mut executor = SessionCheckpointRecorder::default();
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(&recovered, &mut executor)
        .await
        .expect("the first drain finalizes the carrier the earlier release left");
    drop(mutation);
    assert_eq!(
        executor.snapshots,
        vec![committed_snapshot],
        "the drain takes the session checkpoint the committed run still owed"
    );
    assert_committed_run_finalized(&fixture, &input_id).await;
    registration
        .rollback_now()
        .await
        .expect("the healed session's registration is released");
    assert!(!machine.contains_session(&fixture.session_id).await);
    for queued_id in &queued {
        let row = fixture
            .store
            .load_input_state(&fixture.runtime_id(), queued_id)
            .await
            .unwrap()
            .expect("a queued durable input stays stored");
        assert_eq!(
            row.seed.phase,
            InputLifecycleState::Queued,
            "a queued durable input is still queued"
        );
        assert!(
            row.seed.terminal_outcome.is_none(),
            "a queued durable input is still unfinished work"
        );
    }
}

/// One run consumed a durable and an ephemeral input in one boundary, so one
/// owner row carries the completion of both. After a restart the first drain
/// finalizes the whole batch, whichever of the two owns it.
async fn the_first_drain_after_a_restart_finalizes_a_batch_of_mixed_durability(
    owner: InputDurability,
    recipient: InputDurability,
) {
    let fixture = Fixture::with_initial_input(false).await;
    let inputs = [old_input_id(), InputId::new()];
    fixture
        .consume_batch(
            &[(inputs[0].clone(), owner), (inputs[1].clone(), recipient)],
            None,
            None,
        )
        .await;
    let owner_row = fixture.pending(&inputs[0]).await;
    assert_eq!(
        owner_row
            .state
            .terminal_completion
            .as_ref()
            .unwrap()
            .completion_input_ids
            .as_deref(),
        Some(inputs.as_slice()),
    );
    assert_eq!(fixture.stored_durability(&inputs[0]).await, Some(owner));
    assert_eq!(fixture.stored_durability(&inputs[1]).await, Some(recipient));

    let driver = fixture
        .fresh_driver_from_durable()
        .await
        .expect("the next start recovers the store");
    crate::runtime_loop::test_drain_recovered_input_terminal_completions(&driver, &mut NoExecution)
        .await
        .expect("the startup drain finalizes the whole batch");
    fixture.assert_finalized(&inputs[0]).await;
    let second = fixture
        .store
        .load_input_state(&fixture.runtime_id(), &inputs[1])
        .await
        .unwrap()
        .expect("the recipient row stays stored");
    assert_eq!(
        second.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed)
    );
    let completion = second.state.terminal_completion.unwrap();
    assert_eq!(completion.owner_input_id, inputs[0]);
    assert!(matches!(
        completion.phase,
        InputTerminalCompletionPhase::Finalized { .. }
    ));
    assert_eq!(fixture.stored_durability(&inputs[0]).await, Some(owner));
    assert_eq!(fixture.stored_durability(&inputs[1]).await, Some(recipient));
}

#[tokio::test]
async fn a_batch_owned_by_an_ephemeral_input_with_a_durable_recipient_is_finalized_after_a_restart()
{
    the_first_drain_after_a_restart_finalizes_a_batch_of_mixed_durability(
        InputDurability::Ephemeral,
        InputDurability::Durable,
    )
    .await;
}

#[tokio::test]
async fn a_batch_owned_by_a_durable_input_with_an_ephemeral_recipient_is_finalized_after_a_restart()
{
    the_first_drain_after_a_restart_finalizes_a_batch_of_mixed_durability(
        InputDurability::Durable,
        InputDurability::Ephemeral,
    )
    .await;
}

/// Unfinished ephemeral work does not survive a restart: a queued ephemeral
/// input is dropped from the store and from the recovered driver, and a
/// queued durable input beside it is kept.
#[tokio::test]
async fn a_queued_ephemeral_input_is_dropped_at_a_restart_and_a_durable_one_is_kept() {
    let fixture = Fixture::with_initial_input(false).await;
    let kept = InputId::new();
    let dropped = InputId::new();
    {
        let mut entry = fixture.driver.lock().await;
        for (input_id, durability) in [
            (kept.clone(), InputDurability::Durable),
            (dropped.clone(), InputDurability::Ephemeral),
        ] {
            let input = carrier_input(input_id, durability);
            let admission = entry
                .resolve_admission_with_active_turn_boundary(&input, true)
                .unwrap();
            entry.accept_resolved_input(input, admission).await.unwrap();
        }
    }
    for input_id in [&kept, &dropped] {
        let row = fixture
            .store
            .load_input_state(&fixture.runtime_id(), input_id)
            .await
            .unwrap()
            .expect("an admitted input is stored whatever its durability");
        assert!(row.seed.terminal_outcome.is_none());
    }

    let driver = fixture
        .fresh_driver_from_durable()
        .await
        .expect("the next start recovers the store");
    let entry = driver.lock().await;
    assert!(entry.as_driver().stored_input_state(&kept).is_some());
    assert!(entry.as_driver().stored_input_state(&dropped).is_none());
    drop(entry);
    assert!(
        fixture
            .store
            .load_input_state(&fixture.runtime_id(), &kept)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        fixture
            .store
            .load_input_state(&fixture.runtime_id(), &dropped)
            .await
            .unwrap()
            .is_none(),
        "the dropped input's row is deleted by the recovery"
    );
}
