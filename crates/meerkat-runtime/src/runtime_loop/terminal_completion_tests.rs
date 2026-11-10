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

/// Failed attempts that bring a batch's inputs to the stage-attempt cap.
const STAGE_ATTEMPTS_TO_CAP: usize = 3;

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

/// A run of two independent inputs the owned terminalization cancelled.
struct CancelledRun {
    inputs: Vec<InputId>,
    run_id: RunId,
    carrier: Arc<RuntimeLoopTeardownSlot>,
    /// Operation ids of the refusals staged on the first input's live row
    /// before the terminal write, in order.
    recorded: Vec<meerkat_core::OperationId>,
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
        Self::failed_attempts(failure, registered_placement, STAGE_ATTEMPTS_TO_CAP).await
    }

    /// One failed attempt of a fresh batch: its inputs are requeued, not
    /// terminal, and the attempt's teardown carrier holds the failure.
    async fn requeued_attempt_with_cause(failure: CoreApplyFailureCause) -> Self {
        Self::failed_attempts(failure, false, 1).await
    }

    async fn failed_attempts(
        failure: CoreApplyFailureCause,
        registered_placement: bool,
        attempts: usize,
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
        let (inputs, run_id, carrier) = Self::fail_batch_attempts(&driver, failure, attempts).await;
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
        Self::fail_batch_attempts(driver, failure, STAGE_ATTEMPTS_TO_CAP).await
    }

    /// Accept two independent inputs and fail `attempts` batches of them.
    /// The last attempt terminalizes through the owned realization and its
    /// teardown carrier; [`STAGE_ATTEMPTS_TO_CAP`] attempts reach the
    /// stage-attempt cap, fewer leave the inputs requeued.
    async fn fail_batch_attempts(
        driver: &owner::SharedDriver,
        failure: CoreApplyFailureCause,
        attempts: usize,
    ) -> (Vec<InputId>, RunId, Arc<RuntimeLoopTeardownSlot>) {
        let inputs = Self::accept_independent_inputs(driver).await;
        let mut last_run = None;
        let carrier = RuntimeLoopTeardownSlot::pending();
        for attempt in 0..attempts {
            let run_id = Self::start_run(driver, inputs.len()).await;
            if attempt + 1 == attempts {
                let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
                let waited = realize_runtime_loop_terminal_owned(
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
                    None,
                )
                .await
                .unwrap();
                assert!(matches!(
                    waited,
                    OwnedRuntimeLoopChildWait::Settled(
                        _,
                        OwnedRuntimeLoopTerminalizationOutcome::Persisted
                    )
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

    /// Accept two independent queued peer inputs.
    async fn accept_independent_inputs(driver: &owner::SharedDriver) -> Vec<InputId> {
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
                in_reply_to: None,
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
        inputs
    }

    /// Start one run of two independent inputs and cancel it: the owned
    /// terminalization writes both rows `Abandoned { Cancelled }` with their
    /// terminal completion pending, and the run's teardown carrier holds the
    /// persisted non-directed terminal.
    async fn cancelled_run() -> Self {
        let (fixture, recorded) = Self::cancelled_run_recording(0).await;
        assert!(recorded.is_empty());
        fixture
    }

    /// [`Self::cancelled_run`] where an operation of the run stages
    /// `recorded` refusals on the first input's live row before the owned
    /// terminalization, which writes them to the stored row. Returns their
    /// operation ids in order.
    async fn cancelled_run_recording(recorded: usize) -> (Self, Vec<meerkat_core::OperationId>) {
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
        let run = Self::cancel_run_recording(&driver, &session_id, recorded).await;
        let fixture = Self {
            _dir: dir,
            store,
            driver,
            runtime_id,
            session_id,
            inputs: run.inputs,
            run_id: run.run_id,
            carrier: run.carrier,
        };
        assert_eq!(
            crate::input_audit::tests::audit_operation_ids(
                &fixture
                    .stored_row(&fixture.inputs[0])
                    .await
                    .state
                    .authorization_audit
            ),
            run.recorded,
            "the terminal write stores the records staged before it"
        );
        (fixture, run.recorded)
    }

    /// Accept two independent inputs, start their run, let an operation of
    /// the run stage `recorded` refusals on the first input's live row, and
    /// cancel the run through the owned terminalization.
    async fn cancel_run_recording(
        driver: &owner::SharedDriver,
        session_id: &SessionId,
        recorded: usize,
    ) -> CancelledRun {
        let inputs = Self::accept_independent_inputs(driver).await;
        let run_id = Self::start_run(driver, inputs.len()).await;
        let mut operations = Vec::with_capacity(recorded);
        {
            let entry = driver.lock().await;
            let live = &entry
                .shell_driver()
                .ledger()
                .get(&inputs[0])
                .expect("live staged row")
                .authorization_audit;
            for _ in 0..recorded {
                operations.push(crate::input_audit::tests::stage_refusal_after_row_write(
                    live, &inputs[0], session_id, &run_id,
                ));
            }
        }
        let carrier = RuntimeLoopTeardownSlot::pending();
        let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
        let waited = realize_runtime_loop_terminal_owned(
            gate.lock_owned().await,
            carrier.clone(),
            driver.clone(),
            run_id.clone(),
            inputs.clone(),
            false,
            OwnedRuntimeLoopTerminalization::Cancelled,
            None,
        )
        .await
        .unwrap();
        assert!(matches!(
            waited,
            OwnedRuntimeLoopChildWait::Settled(
                _,
                OwnedRuntimeLoopTerminalizationOutcome::Persisted
            )
        ));
        CancelledRun {
            inputs,
            run_id,
            carrier,
            recorded: operations,
        }
    }

    /// Authorize the next runtime-loop batch, which must hold `inputs`
    /// inputs, and start its run.
    async fn start_run(driver: &owner::SharedDriver, inputs: usize) -> RunId {
        let batch = owner::machine_authorize_runtime_loop_batch(&*driver.lock().await).unwrap();
        assert_eq!(batch.input_ids().len(), inputs);
        let run_id = RunId::new();
        assert!(matches!(
            owner::prepare_runtime_loop_batch_start(
                driver,
                run_id.clone(),
                batch,
                crate::meerkat_machine::RunlessTerminalAnnouncement::default()
            )
            .await
            .unwrap(),
            owner::RuntimeLoopBatchStart::Started
        ));
        run_id
    }

    /// An operation of the cancelled run stages a refusal on the first
    /// input's live row after the owned terminalization wrote that row.
    async fn stage_refusal_after_terminal_write(&self) -> meerkat_core::OperationId {
        let before = self.raw_row(&self.inputs[0]);
        let entry = self.driver.lock().await;
        let observed = crate::input_audit::tests::stage_refusal_after_row_write(
            &entry
                .shell_driver()
                .ledger()
                .get(&self.inputs[0])
                .expect("live terminal row")
                .authorization_audit,
            &self.inputs[0],
            &self.session_id,
            &self.run_id,
        );
        drop(entry);
        assert_eq!(
            before,
            self.raw_row(&self.inputs[0]),
            "staging writes no row"
        );
        observed
    }

    /// Both rows finalized as the cancelled run, no pending terminal owner
    /// left, and the first row's durable audit holding exactly `first_audit`,
    /// in order.
    async fn assert_cancelled_finalized(&self, first_audit: &[meerkat_core::OperationId]) {
        for input_id in &self.inputs {
            let row = self
                .store
                .load_input_state(&self.runtime_id, input_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                row.seed.terminal_outcome,
                Some(crate::input_state::InputTerminalOutcome::Abandoned {
                    reason: crate::input_state::InputAbandonReason::Cancelled
                })
            );
            let completion = row.state.terminal_completion.as_ref().unwrap();
            assert_eq!(completion.batch_key.run_id(), Some(&self.run_id));
            assert!(matches!(
                completion.phase,
                InputTerminalCompletionPhase::Finalized { .. }
            ));
            if completion.owner_input_id == *input_id {
                assert!(matches!(
                    completion.outcome,
                    Some(crate::completion::CompletionOutcome::Cancelled)
                ));
            } else {
                assert!(completion.outcome.is_none());
            }
            let expected_audit = if *input_id == self.inputs[0] {
                first_audit.to_vec()
            } else {
                Vec::new()
            };
            assert_eq!(
                crate::input_audit::tests::audit_operation_ids(&row.state.authorization_audit),
                expected_audit,
                "the finalization commits every observation staged before it"
            );
        }
        let pending_owners: usize = rusqlite::Connection::open(self.store.path())
            .unwrap()
            .query_row(
                "SELECT count(*) FROM runtime_pending_terminal_owners WHERE runtime_id=?1",
                rusqlite::params![self.runtime_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pending_owners, 0);
    }

    async fn stored_row(&self, input_id: &InputId) -> crate::input_state::StoredInputState {
        self.store
            .load_input_state(&self.runtime_id, input_id)
            .await
            .unwrap()
            .unwrap()
    }

    /// Write the first input's stored row as another writer does. Returns
    /// the bytes written.
    fn replace_first_stored_row(&self, row: &crate::input_state::StoredInputState) -> Vec<u8> {
        let bytes = serde_json::to_vec(row).unwrap();
        rusqlite::Connection::open(self.store.path())
            .unwrap()
            .execute(
                "UPDATE runtime_input_states SET state_json=?3 WHERE runtime_id=?1 AND input_id=?2",
                rusqlite::params![
                    self.runtime_id.to_string(),
                    self.inputs[0].to_string(),
                    &bytes
                ],
            )
            .unwrap();
        bytes
    }

    /// Teardown's recovery of the run refuses the first stored row the other
    /// writer left as `written`, writes neither row, and keeps the carrier.
    async fn assert_teardown_refuses_changed_first_row(&self, written: &[u8]) {
        let second = self.raw_row(&self.inputs[1]);
        let recovery = RuntimeLoopRecoveryContext {
            authority_binding: RuntimeLoopAuthorityBinding::detached_for_test(),
            completions: Some(Arc::new(crate::tokio::sync::Mutex::new(
                crate::completion::CompletionRegistry::new(),
            ))),
        };
        let error = recover_committed_runtime_projections_before_teardown(
            &self.driver,
            &mut NoExecution,
            &self.carrier,
            &recovery,
        )
        .await
        .expect_err("a stored row that differs beyond the live audit suffix stays stale");
        assert!(
            error
                .to_string()
                .contains("terminal completion finalization lost the exact durable CAS"),
            "{error}"
        );
        assert_eq!(written, self.raw_row(&self.inputs[0]).as_slice());
        assert_eq!(second, self.raw_row(&self.inputs[1]));
        assert!(self.carrier.pending_nondirected_run_terminal().is_some());
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
                residency_owner: mm::SessionResidencyOwner::Session,
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
        in_reply_to: None,
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
            crate::meerkat_machine::RunlessTerminalAnnouncement::default(),
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

/// A failure that keeps a typed reason: a scoped tool grant gate the
/// session's build refused.
fn grant_gate_refused_failure() -> CoreApplyFailureCause {
    CoreApplyFailureCause::runtime_turn(FAILURE).with_reason(
        meerkat_core::event::AgentErrorReason::ToolGrantGateRefused {
            refusal: meerkat_core::tool_grant::GrantGateRefusal::UnknownFamily {
                family: "shel".to_string(),
            },
        },
    )
}

/// The failed-turn error `failure` is staged and delivered with: the runtime
/// apply failure over its message, with its typed reason.
fn staged_error_of(failure: &CoreApplyFailureCause) -> meerkat_core::TurnErrorMetadata {
    let mut error = meerkat_core::TurnErrorMetadata::runtime_apply_failure(failure.message());
    error.reason = failure.reason().cloned();
    error
}

/// A failure that keeps a typed reason reaches the stage-attempt cap with
/// it: the realization stages the reason on the batch's terminal candidate
/// and on the teardown carrier, and the waiters the same cause's detail and
/// reason resolve finalize the batch and receive it.
#[tokio::test]
async fn failed_completion_stages_and_delivers_the_apply_failure_reason() {
    let failure = grant_gate_refused_failure();
    let expected = staged_error_of(&failure);
    assert!(
        expected.reason.is_some(),
        "the fixture's cause keeps a reason"
    );
    let fixture = Fixture::failed_batch_with_cause(failure.clone()).await;
    fixture.pending().await;

    let row = fixture
        .store
        .load_input_state(&fixture.runtime_id, &fixture.inputs[0])
        .await
        .unwrap()
        .unwrap();
    let owner_id = row.state.terminal_completion.unwrap().owner_input_id;
    let owner_row = fixture
        .store
        .load_input_state(&fixture.runtime_id, &owner_id)
        .await
        .unwrap()
        .unwrap();
    let candidate = owner_row
        .state
        .terminal_completion
        .unwrap()
        .candidate
        .expect("the owner row holds the batch's candidate");
    assert_eq!(
        candidate.completion_error_metadata(),
        Some(expected.clone()),
        "the realization stages the cause's typed reason on the batch's candidate"
    );
    let pending = fixture.carrier.pending_nondirected_run_terminal().unwrap();
    assert_eq!(
        pending.completion_error_metadata,
        Some(expected.clone()),
        "the teardown carrier keeps the cause's typed reason"
    );

    let completions = Arc::new(crate::tokio::sync::Mutex::new(
        crate::completion::CompletionRegistry::new(),
    ));
    let mut handles = Vec::new();
    for id in &fixture.inputs {
        handles.push(completions.lock().await.register(id.clone()));
    }
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    resolve_machine_terminal_completion_waiters_under_authority(
        &fixture.driver,
        Some(&completions),
        gate.lock_owned().await,
        &fixture.carrier,
        &fixture.inputs,
        &fixture.run_id,
        RunFailureCompletionDetail::of_apply_failure(&failure),
    )
    .await
    .expect("the cause's detail and reason finalize the staged batch");
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    for handle in handles {
        let outcome = handle.wait_authorized().await;
        let crate::completion::CompletionOutcome::AbandonedWithError { error, .. } = &outcome
        else {
            panic!("expected the failed attempt, got {outcome:?}");
        };
        assert_eq!(error, &expected, "the waiter receives the typed reason");
    }
}

/// Abnormal teardown recovers a failed attempt's waiters from its carrier
/// with the failure's typed reason. The attempt requeued its inputs, so the
/// waiters are process-local: they receive the refusal the live path gives
/// them, never a reason-less runtime apply failure.
#[tokio::test]
async fn teardown_recovery_delivers_the_carried_apply_failure_reason() {
    let failure = grant_gate_refused_failure();
    let expected = staged_error_of(&failure);
    let fixture = Fixture::requeued_attempt_with_cause(failure).await;
    let pending = fixture
        .carrier
        .pending_nondirected_run_terminal()
        .expect("the failed attempt keeps its teardown carrier");
    assert_eq!(pending.completion_error_metadata, Some(expected.clone()));

    let completions = Arc::new(crate::tokio::sync::Mutex::new(
        crate::completion::CompletionRegistry::new(),
    ));
    let mut handles = Vec::new();
    for id in &fixture.inputs {
        handles.push(completions.lock().await.register(id.clone()));
    }
    let recovery = RuntimeLoopRecoveryContext {
        authority_binding: RuntimeLoopAuthorityBinding::detached_for_test(),
        completions: Some(Arc::clone(&completions)),
    };
    recover_committed_runtime_projections_before_teardown(
        &fixture.driver,
        &mut NoExecution,
        &fixture.carrier,
        &recovery,
    )
    .await
    .expect("teardown recovers the failed attempt's waiters");
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    for handle in handles {
        let outcome = handle.wait_authorized().await;
        let crate::completion::CompletionOutcome::AbandonedWithError { error, .. } = &outcome
        else {
            panic!("expected the failed attempt, got {outcome:?}");
        };
        assert_eq!(
            error, &expected,
            "teardown recovery delivers the carried typed reason"
        );
    }
}

/// Abnormal teardown settles a cancelled run's terminal when an operation of
/// the run staged an audit observation after the owned terminalization wrote
/// the run's rows. The observation waits on the live row for the row's next
/// update; the finalization is that update: it compares against the rows as
/// last written and commits the observation with the receipt.
#[tokio::test]
async fn teardown_recovery_finalizes_a_cancelled_run_after_a_late_audit_observation() {
    let fixture = Fixture::cancelled_run().await;
    fixture.pending().await;
    let observed = fixture.stage_refusal_after_terminal_write().await;
    let completions = Arc::new(crate::tokio::sync::Mutex::new(
        crate::completion::CompletionRegistry::new(),
    ));
    let mut handles = Vec::new();
    for id in &fixture.inputs {
        handles.push(completions.lock().await.register(id.clone()));
    }
    let recovery = RuntimeLoopRecoveryContext {
        authority_binding: RuntimeLoopAuthorityBinding::detached_for_test(),
        completions: Some(Arc::clone(&completions)),
    };
    recover_committed_runtime_projections_before_teardown(
        &fixture.driver,
        &mut NoExecution,
        &fixture.carrier,
        &recovery,
    )
    .await
    .expect("teardown settles the cancelled run's terminal");
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    for handle in handles {
        let outcome =
            tokio::time::timeout(std::time::Duration::from_secs(10), handle.wait_authorized())
                .await
                .expect("teardown resolves every waiter of the cancelled run");
        assert!(
            matches!(outcome, crate::completion::CompletionOutcome::Cancelled),
            "expected the cancelled run, got {outcome:?}"
        );
    }
    fixture
        .assert_cancelled_finalized(std::slice::from_ref(&observed))
        .await;
}

/// The live path settles the same way: the runtime loop's cancelled-run
/// finalization commits an observation staged after the terminal write,
/// clears the run's teardown carrier, and the runtime starts its next run.
#[tokio::test]
async fn live_cancelled_finalization_commits_a_late_audit_observation_and_runs_on() {
    let fixture = Fixture::cancelled_run().await;
    let observed = fixture.stage_refusal_after_terminal_write().await;
    let completions = Arc::new(crate::tokio::sync::Mutex::new(
        crate::completion::CompletionRegistry::new(),
    ));
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    finalize_publish_and_resolve_runtime_terminals(
        &fixture.driver,
        &Arc::new(meerkat_core::PostCommitHookDispatcher::new(
            fixture.session_id.clone(),
        )),
        Some(&completions),
        gate.lock_owned().await,
        &fixture.carrier,
        &mut NoExecution,
        &fixture.inputs,
        &[],
        &fixture.run_id,
        mm::RuntimeCompletionTerminalObservation::MachineTerminal,
        None,
        mm::RuntimeCompletionFinalizationObservation::Succeeded,
        None,
    )
    .await
    .expect("the live cancelled path finalizes the run's terminal");
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    fixture
        .assert_cancelled_finalized(std::slice::from_ref(&observed))
        .await;
    let next = Fixture::accept_independent_inputs(&fixture.driver).await;
    Fixture::start_run(&fixture.driver, next.len()).await;
}

/// Expecting each row as it was last written accepts only the live audit
/// suffix: with an observation waiting on the live row, the finalization
/// still refuses a stored row another writer changed, and leaves that row as
/// the writer left it.
#[tokio::test]
async fn late_audit_observation_does_not_excuse_a_changed_stored_row() {
    let fixture = Fixture::cancelled_run().await;
    fixture.stage_refusal_after_terminal_write().await;
    let mut changed = fixture.stored_row(&fixture.inputs[0]).await;
    changed.state.updated_at += chrono::Duration::seconds(1);
    let written = fixture.replace_first_stored_row(&changed);
    fixture
        .assert_teardown_refuses_changed_first_row(&written)
        .await;
}

/// The same for a stored audit that is no prefix of the live one: a record
/// the live row never held stays a difference.
#[tokio::test]
async fn late_audit_observation_does_not_excuse_a_divergent_stored_audit() {
    let fixture = Fixture::cancelled_run().await;
    fixture.stage_refusal_after_terminal_write().await;
    let mut divergent = fixture.stored_row(&fixture.inputs[0]).await;
    let other = crate::input_audit::InputAuthorizationAudit::default();
    crate::input_audit::tests::stage_refusal_after_row_write(
        &other,
        &fixture.inputs[0],
        &fixture.session_id,
        &fixture.run_id,
    );
    divergent.state.authorization_audit = other;
    let written = fixture.replace_first_stored_row(&divergent);
    fixture
        .assert_teardown_refuses_changed_first_row(&written)
        .await;
}

/// A stored audit shorter than the live one that is no prefix of it (a
/// record the live row never held, in place of the one the row was written
/// with) stays a difference: only the live row's own stored records are
/// expected.
#[tokio::test]
async fn late_audit_observation_does_not_excuse_a_shorter_stored_audit_that_is_no_prefix() {
    let (fixture, recorded) = Fixture::cancelled_run_recording(1).await;
    assert_eq!(recorded.len(), 1);
    fixture.stage_refusal_after_terminal_write().await;
    let mut shorter = fixture.stored_row(&fixture.inputs[0]).await;
    let other = crate::input_audit::InputAuthorizationAudit::default();
    crate::input_audit::tests::stage_refusal_after_row_write(
        &other,
        &fixture.inputs[0],
        &fixture.session_id,
        &fixture.run_id,
    );
    shorter.state.authorization_audit = other;
    let written = fixture.replace_first_stored_row(&shorter);
    fixture
        .assert_teardown_refuses_changed_first_row(&written)
        .await;
}

/// A stored audit longer than the live one (the live records plus one the
/// live row lacks, as if another writer extended it) stays a difference: the
/// rewrite would drop that record.
#[tokio::test]
async fn late_audit_observation_does_not_excuse_a_longer_stored_audit() {
    let (fixture, recorded) = Fixture::cancelled_run_recording(1).await;
    assert_eq!(recorded.len(), 1);
    fixture.stage_refusal_after_terminal_write().await;
    let extended = {
        let entry = fixture.driver.lock().await;
        entry
            .shell_driver()
            .ledger()
            .get(&fixture.inputs[0])
            .expect("live terminal row")
            .authorization_audit
            .freeze_for_persistence()
            .unwrap()
            .restore_observations_for_owner()
            .unwrap()
    };
    crate::input_audit::tests::stage_refusal_after_row_write(
        &extended,
        &fixture.inputs[0],
        &fixture.session_id,
        &fixture.run_id,
    );
    let mut longer = fixture.stored_row(&fixture.inputs[0]).await;
    longer.state.authorization_audit = extended;
    let written = fixture.replace_first_stored_row(&longer);
    fixture
        .assert_teardown_refuses_changed_first_row(&written)
        .await;
}

/// The field shape: the stored row already holds records an operation of the
/// run staged before the terminal write, and one more waits on the live row.
/// Teardown expects the stored records, settles the run, and commits every
/// record in order.
#[tokio::test]
async fn teardown_recovery_finalizes_over_a_stored_audit_prefix() {
    let (fixture, mut records) = Fixture::cancelled_run_recording(3).await;
    records.push(fixture.stage_refusal_after_terminal_write().await);
    let recovery = RuntimeLoopRecoveryContext {
        authority_binding: RuntimeLoopAuthorityBinding::detached_for_test(),
        completions: Some(Arc::new(crate::tokio::sync::Mutex::new(
            crate::completion::CompletionRegistry::new(),
        ))),
    };
    recover_committed_runtime_projections_before_teardown(
        &fixture.driver,
        &mut NoExecution,
        &fixture.carrier,
        &recovery,
    )
    .await
    .expect("teardown settles the cancelled run over the stored records");
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    fixture.assert_cancelled_finalized(&records).await;
}

/// [`Fixture::cancelled_run_recording`] on the in-memory store, whose test
/// hooks hold a terminal input CAS right after its commit. The driver carries
/// the session's durability gate, as production persistent drivers do.
struct MemoryFixture {
    store: Arc<crate::store::InMemoryRuntimeStore>,
    driver: owner::SharedDriver,
    health: crate::meerkat_machine::DurabilityHealthHandle,
    runtime_id: crate::identifiers::LogicalRuntimeId,
    session_id: SessionId,
    inputs: Vec<InputId>,
    run_id: RunId,
    carrier: Arc<RuntimeLoopTeardownSlot>,
}

impl MemoryFixture {
    async fn cancelled_run_recording(recorded: usize) -> (Self, Vec<meerkat_core::OperationId>) {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let session_id = SessionId::new();
        let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
        let health = owner::ready_durability_health_for_test();
        let driver = Arc::new(crate::tokio::sync::Mutex::new(
            owner::DriverEntry::Persistent(
                PersistentRuntimeDriver::new_with_control_and_durability_health(
                    runtime_id.clone(),
                    store.clone(),
                    Arc::new(meerkat_store::MemoryBlobStore::new()),
                    Arc::new(std::sync::RwLock::new(
                        crate::driver::ephemeral::RuntimeControlProjection::default(),
                    )),
                    crate::driver::ephemeral::new_ingress_dsl_authority(),
                    health.clone(),
                ),
            ),
        ));
        let run = Fixture::cancel_run_recording(&driver, &session_id, recorded).await;
        let fixture = Self {
            store,
            driver,
            health,
            runtime_id,
            session_id,
            inputs: run.inputs,
            run_id: run.run_id,
            carrier: run.carrier,
        };
        assert_eq!(
            crate::input_audit::tests::audit_operation_ids(
                &fixture
                    .stored_row(&fixture.inputs[0])
                    .await
                    .state
                    .authorization_audit
            ),
            run.recorded,
            "the terminal write stores the records staged before it"
        );
        (fixture, run.recorded)
    }

    /// The first input's live audit, which an operation of the run stages on.
    async fn first_live_audit(&self) -> crate::input_audit::InputAuthorizationAudit {
        self.driver
            .lock()
            .await
            .shell_driver()
            .ledger()
            .get(&self.inputs[0])
            .expect("live terminal row")
            .authorization_audit
            .clone()
    }

    /// An operation of the cancelled run stages a refusal on `live`, the
    /// first input's live audit.
    fn stage_refusal(
        &self,
        live: &crate::input_audit::InputAuthorizationAudit,
    ) -> meerkat_core::OperationId {
        crate::input_audit::tests::stage_refusal_after_row_write(
            live,
            &self.inputs[0],
            &self.session_id,
            &self.run_id,
        )
    }

    /// Teardown's recovery of the cancelled run's persisted terminal.
    async fn teardown(&self) -> Result<(), crate::RuntimeDriverError> {
        let recovery = RuntimeLoopRecoveryContext {
            authority_binding: RuntimeLoopAuthorityBinding::detached_for_test(),
            completions: None,
        };
        recover_committed_runtime_projections_before_teardown(
            &self.driver,
            &mut NoExecution,
            &self.carrier,
            &recovery,
        )
        .await
    }

    /// Hold the next terminal input CAS right after the store commits it,
    /// until `release` is notified. Returns `(entered, release)`.
    fn hold_next_cas_after_commit(
        &self,
    ) -> (
        Arc<crate::tokio::sync::Notify>,
        Arc<crate::tokio::sync::Notify>,
    ) {
        let entered = Arc::new(crate::tokio::sync::Notify::new());
        let release = Arc::new(crate::tokio::sync::Notify::new());
        self.store
            .block_next_input_state_batch_cas_after_commit(entered.clone(), release.clone());
        (entered, release)
    }

    /// Finalize the cancelled run's terminal as teardown does, and drop the
    /// finalization right after the store committed its CAS, before the
    /// driver saw the outcome: the stored rows are finalized while the live
    /// rows stay as the finalization found them.
    async fn drop_finalization_after_its_cas_commits(&self) {
        let pending = self
            .carrier
            .pending_nondirected_run_terminal()
            .expect("the cancelled run's teardown carrier");
        let (entered, _release) = self.hold_next_cas_after_commit();
        let driver = self.driver.clone();
        let finalization = crate::tokio::spawn(async move {
            let mut entry = driver.lock_owned().await;
            let witness = entry
                .input_terminal_completion_authorization_witness(&pending.input_ids)
                .unwrap();
            let authority = owner::machine_resolve_runtime_completion_result_for_batch(
                &entry,
                &witness,
                Some(&pending.run_id),
                pending.terminal_observation,
                mm::RuntimeCompletionFinalizationObservation::Succeeded,
            )
            .unwrap();
            let bundle = crate::completion::authorize_runtime_terminal_bundle(
                &[],
                pending.terminal.as_ref(),
                authority,
                witness,
                pending.completion_error_metadata,
                None,
            )
            .unwrap();
            entry
                .finalize_input_terminal_completion_batch(bundle.terminal_completion())
                .await
        });
        crate::tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .expect("the finalization CAS commits");
        finalization.abort();
        assert!(
            finalization.await.unwrap_err().is_cancelled(),
            "the finalization is dropped while its CAS is held after the commit"
        );
        let entry = self.driver.lock().await;
        for input_id in &self.inputs {
            let stored = self.stored_row(input_id).await;
            assert!(matches!(
                stored.state.terminal_completion.unwrap().phase,
                InputTerminalCompletionPhase::Finalized { .. }
            ));
            let live = entry
                .as_driver()
                .stored_input_state(input_id)
                .expect("the shell keeps the replaced row");
            assert!(matches!(
                live.state.terminal_completion.unwrap().phase,
                InputTerminalCompletionPhase::Pending
            ));
        }
    }

    async fn stored_row(&self, input_id: &InputId) -> crate::input_state::StoredInputState {
        self.store
            .load_input_state(&self.runtime_id, input_id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn stored_bytes(&self, input_id: &InputId) -> Vec<u8> {
        serde_json::to_vec(&self.stored_row(input_id).await).unwrap()
    }

    /// Write the first input's stored row as another writer does.
    async fn replace_first_stored_row(&self, row: crate::input_state::StoredInputState) {
        self.store
            .persist_input_state(
                &self.runtime_id,
                &crate::input_state::InputStatePersistenceRecord::stored_row_for_test(row).unwrap(),
            )
            .await
            .unwrap();
    }

    /// Teardown's recovery refuses the stored rows as they are, writes
    /// neither, and keeps the carrier.
    async fn assert_teardown_refuses_stored_rows(&self) {
        let first = self.stored_bytes(&self.inputs[0]).await;
        let second = self.stored_bytes(&self.inputs[1]).await;
        let error = self
            .teardown()
            .await
            .expect_err("a stored row that differs beyond an audit prefix stays stale");
        assert!(
            error
                .to_string()
                .contains("terminal completion finalization lost the exact durable CAS"),
            "{error}"
        );
        assert_eq!(first, self.stored_bytes(&self.inputs[0]).await);
        assert_eq!(second, self.stored_bytes(&self.inputs[1]).await);
        assert!(self.carrier.pending_nondirected_run_terminal().is_some());
    }

    /// While the finalization's CAS is held right after its commit (`entered`,
    /// `release` from [`Self::hold_next_cas_after_commit`]), an operation of
    /// the run stages a refusal on `live` and another writer changes the first
    /// stored row; then the hook releases. Returns the refusal's operation id
    /// and both stored rows' bytes as the writers left them.
    async fn stage_and_change_first_row_after_the_finalization_cas(
        &self,
        live: &crate::input_audit::InputAuthorizationAudit,
        entered: Arc<crate::tokio::sync::Notify>,
        release: Arc<crate::tokio::sync::Notify>,
    ) -> (meerkat_core::OperationId, [Vec<u8>; 2]) {
        crate::tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .expect("the finalization CAS commits");
        let late = self.stage_refusal(live);
        let mut changed = self.stored_row(&self.inputs[0]).await;
        changed.state.updated_at += chrono::Duration::seconds(1);
        self.replace_first_stored_row(changed).await;
        let written = [
            self.stored_bytes(&self.inputs[0]).await,
            self.stored_bytes(&self.inputs[1]).await,
        ];
        release.notify_one();
        (late, written)
    }

    /// The archive refused the stored rows the writers left as `written`: the
    /// session is marked for a cold reload with the archive's reason, neither
    /// stored row was written again (so `late` is not stored), and both rows
    /// stay live, never archived over the refusal.
    async fn assert_archive_refused_for_a_cold_reload(
        &self,
        written: &[Vec<u8>; 2],
        late: &meerkat_core::OperationId,
    ) {
        let required = self
            .health
            .require_ready()
            .expect_err("a refused archive marks the session for a cold reload");
        assert_eq!(required.operation(), "terminal_receipt_archive");
        assert!(
            required
                .reason()
                .contains("terminal input archive lost the exact durable CAS"),
            "{required}"
        );
        assert_eq!(written[0], self.stored_bytes(&self.inputs[0]).await);
        assert_eq!(written[1], self.stored_bytes(&self.inputs[1]).await);
        assert!(
            !crate::input_audit::tests::audit_operation_ids(
                &self
                    .stored_row(&self.inputs[0])
                    .await
                    .state
                    .authorization_audit
            )
            .contains(late),
            "the refused archive stores nothing"
        );
        let entry = self.driver.lock().await;
        for input_id in &self.inputs {
            assert!(
                entry.shell_driver().ledger().get(input_id).is_some(),
                "the refused rows are not archived"
            );
        }
    }

    /// Both rows finalized as the cancelled run and archived from the live
    /// shell, no pending terminal owner left, and the first row's durable
    /// audit holding exactly `first_audit`, in order.
    async fn assert_cancelled_finalized(&self, first_audit: &[meerkat_core::OperationId]) {
        for input_id in &self.inputs {
            let row = self.stored_row(input_id).await;
            assert_eq!(
                row.seed.terminal_outcome,
                Some(crate::input_state::InputTerminalOutcome::Abandoned {
                    reason: crate::input_state::InputAbandonReason::Cancelled
                })
            );
            let completion = row.state.terminal_completion.as_ref().unwrap();
            assert_eq!(completion.batch_key.run_id(), Some(&self.run_id));
            assert!(matches!(
                completion.phase,
                InputTerminalCompletionPhase::Finalized { .. }
            ));
            if completion.owner_input_id == *input_id {
                assert!(matches!(
                    completion.outcome,
                    Some(crate::completion::CompletionOutcome::Cancelled)
                ));
            } else {
                assert!(completion.outcome.is_none());
            }
            let expected_audit = if *input_id == self.inputs[0] {
                first_audit.to_vec()
            } else {
                Vec::new()
            };
            assert_eq!(
                crate::input_audit::tests::audit_operation_ids(&row.state.authorization_audit),
                expected_audit,
                "every observation staged before the archive is durable"
            );
        }
        {
            let entry = self.driver.lock().await;
            for input_id in &self.inputs {
                assert!(
                    entry.shell_driver().ledger().get(input_id).is_none(),
                    "the finalized rows are archived"
                );
            }
        }
        assert!(
            self.store
                .load_pending_terminal_owner_ids_page(
                    &self.runtime_id,
                    None,
                    crate::store::MAX_PENDING_TERMINAL_OWNER_PAGE,
                )
                .await
                .unwrap()
                .is_empty()
        );
    }
}

/// An operation of the cancelled run stages an observation after the
/// finalization's replacement was frozen and committed, and before the
/// finalized rows are archived. The observation waits on the live row for
/// the row's next write; the archive leaves no next write, so it commits the
/// observation to the stored row first.
#[tokio::test]
async fn teardown_archive_commits_an_audit_observation_staged_after_the_finalization_cas() {
    let (fixture, mut records) = MemoryFixture::cancelled_run_recording(1).await;
    let live = fixture.first_live_audit().await;
    let (entered, release) = fixture.hold_next_cas_after_commit();
    let stage = async {
        crate::tokio::time::timeout(std::time::Duration::from_secs(5), entered.notified())
            .await
            .expect("the finalization CAS commits");
        assert!(
            matches!(
                fixture
                    .stored_row(&fixture.inputs[0])
                    .await
                    .state
                    .terminal_completion
                    .unwrap()
                    .phase,
                InputTerminalCompletionPhase::Finalized { .. }
            ),
            "the hook holds the finalization's own CAS after its commit"
        );
        let observed = fixture.stage_refusal(&live);
        release.notify_one();
        observed
    };
    let (settled, observed) = crate::tokio::join!(fixture.teardown(), stage);
    settled.expect("teardown settles the cancelled run's terminal");
    records.push(observed);
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    fixture.assert_cancelled_finalized(&records).await;
}

/// An observation staged while the archive's own audit commit is held after
/// its commit is checked for again: the archive commits it too before the
/// rows go.
#[tokio::test]
async fn teardown_archive_commits_audit_observations_staged_during_its_own_audit_commit() {
    let (fixture, mut records) = MemoryFixture::cancelled_run_recording(1).await;
    let live = fixture.first_live_audit().await;
    let (finalization_entered, finalization_release) = fixture.hold_next_cas_after_commit();
    let stage = async {
        crate::tokio::time::timeout(
            std::time::Duration::from_secs(5),
            finalization_entered.notified(),
        )
        .await
        .expect("the finalization CAS commits");
        let first = fixture.stage_refusal(&live);
        let (archive_entered, archive_release) = fixture.hold_next_cas_after_commit();
        finalization_release.notify_one();
        crate::tokio::time::timeout(
            std::time::Duration::from_secs(5),
            archive_entered.notified(),
        )
        .await
        .expect("the archive commits the first observation");
        assert_eq!(
            crate::input_audit::tests::audit_operation_ids(
                &fixture
                    .stored_row(&fixture.inputs[0])
                    .await
                    .state
                    .authorization_audit
            )
            .last(),
            Some(&first),
            "the held CAS is the archive's audit commit"
        );
        let second = fixture.stage_refusal(&live);
        archive_release.notify_one();
        [first, second]
    };
    let (settled, observed) = crate::tokio::join!(fixture.teardown(), stage);
    settled.expect("teardown settles the cancelled run's terminal");
    records.extend(observed);
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    fixture.assert_cancelled_finalized(&records).await;
}

/// The archive's audit commit expects every other byte of the row as it was
/// written: a stored row another writer changed after the finalization stays
/// a difference, and the archive refuses. The finalization's receipt is
/// durable by then, so teardown still settles the run's terminal and releases
/// its carrier; the archive stays fail closed: the session is marked for a
/// cold reload, neither stored row is written, and the rows stay live.
#[tokio::test]
async fn teardown_archive_does_not_excuse_a_changed_stored_row() {
    let (fixture, _) = MemoryFixture::cancelled_run_recording(1).await;
    let live = fixture.first_live_audit().await;
    let (entered, release) = fixture.hold_next_cas_after_commit();
    let stage =
        fixture.stage_and_change_first_row_after_the_finalization_cas(&live, entered, release);
    let (settled, (late, written)) = crate::tokio::join!(fixture.teardown(), stage);
    settled.expect("teardown settles the run's terminal once its receipt is durable");
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    fixture
        .assert_archive_refused_for_a_cold_reload(&written, &late)
        .await;
}

/// The live path the same way, with completion waiters: the archive's audit
/// commit runs after the finalization's receipt is durable and realized, so
/// its refusal cannot fail the finalization. The finalization returns, the
/// run's waiters receive the cancelled run, and its carrier is released; the
/// archive stays fail closed and marks the session for a cold reload.
#[tokio::test]
async fn live_finalization_delivers_its_waiters_when_the_archive_audit_commit_is_refused() {
    let (fixture, _) = MemoryFixture::cancelled_run_recording(1).await;
    let live = fixture.first_live_audit().await;
    let completions = Arc::new(crate::tokio::sync::Mutex::new(
        crate::completion::CompletionRegistry::new(),
    ));
    let mut handles = Vec::new();
    for input_id in &fixture.inputs {
        handles.push(completions.lock().await.register(input_id.clone()));
    }
    let hooks = Arc::new(meerkat_core::PostCommitHookDispatcher::new(
        fixture.session_id.clone(),
    ));
    let gate = Arc::new(crate::tokio::sync::Mutex::new(()));
    let authority = gate.lock_owned().await;
    let mut executor = NoExecution;
    let (entered, release) = fixture.hold_next_cas_after_commit();
    let finalize = finalize_publish_and_resolve_runtime_terminals(
        &fixture.driver,
        &hooks,
        Some(&completions),
        authority,
        &fixture.carrier,
        &mut executor,
        &fixture.inputs,
        &[],
        &fixture.run_id,
        mm::RuntimeCompletionTerminalObservation::MachineTerminal,
        None,
        mm::RuntimeCompletionFinalizationObservation::Succeeded,
        None,
    );
    let stage =
        fixture.stage_and_change_first_row_after_the_finalization_cas(&live, entered, release);
    let (finalized, (late, written)) = crate::tokio::join!(finalize, stage);
    finalized.expect("the finalization returns once its receipt is durable");
    for handle in handles {
        let outcome =
            tokio::time::timeout(std::time::Duration::from_secs(10), handle.wait_authorized())
                .await
                .expect("the finalization delivers every waiter of the cancelled run");
        assert!(
            matches!(outcome, crate::completion::CompletionOutcome::Cancelled),
            "expected the cancelled run, got {outcome:?}"
        );
    }
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    fixture
        .assert_archive_refused_for_a_cold_reload(&written, &late)
        .await;
}

/// A finalization dropped right after the store committed its CAS leaves the
/// stored rows finalized and the live rows as they were; an operation of the
/// run then stages an observation on the live row. The retry rebuilds the
/// same finalization from the live rows, observation included: the stored
/// row is that replacement with its audit cut to the records it was written
/// with, which the retry accepts as already committed, and the observation
/// commits.
#[tokio::test]
async fn retry_after_a_dropped_finalization_commits_a_late_audit_observation() {
    let (fixture, mut records) = MemoryFixture::cancelled_run_recording(1).await;
    fixture.drop_finalization_after_its_cas_commits().await;
    records.push(fixture.stage_refusal(&fixture.first_live_audit().await));
    fixture
        .teardown()
        .await
        .expect("teardown settles the finalization its dropped caller committed");
    assert!(fixture.carrier.pending_nondirected_run_terminal().is_none());
    fixture.assert_cancelled_finalized(&records).await;
}

/// After a dropped finalization, a stored audit shorter than the live one
/// that is no prefix of it (a record the live row never held, in place of
/// the one the row was written with) stays a difference.
#[tokio::test]
async fn retry_after_a_dropped_finalization_does_not_excuse_a_shorter_stored_audit_that_is_no_prefix()
 {
    let (fixture, recorded) = MemoryFixture::cancelled_run_recording(1).await;
    assert_eq!(recorded.len(), 1);
    fixture.drop_finalization_after_its_cas_commits().await;
    fixture.stage_refusal(&fixture.first_live_audit().await);
    let mut shorter = fixture.stored_row(&fixture.inputs[0]).await;
    let other = crate::input_audit::InputAuthorizationAudit::default();
    fixture.stage_refusal(&other);
    shorter.state.authorization_audit = other;
    fixture.replace_first_stored_row(shorter).await;
    fixture.assert_teardown_refuses_stored_rows().await;
}

/// After a dropped finalization, a stored audit longer than the live one
/// (the live records plus one the live row lacks, as if another writer
/// extended it) stays a difference: the rewrite would drop that record.
#[tokio::test]
async fn retry_after_a_dropped_finalization_does_not_excuse_a_longer_stored_audit() {
    let (fixture, _) = MemoryFixture::cancelled_run_recording(1).await;
    fixture.drop_finalization_after_its_cas_commits().await;
    let live = fixture.first_live_audit().await;
    fixture.stage_refusal(&live);
    let extended = live
        .freeze_for_persistence()
        .unwrap()
        .restore_observations_for_owner()
        .unwrap();
    fixture.stage_refusal(&extended);
    let mut longer = fixture.stored_row(&fixture.inputs[0]).await;
    longer.state.authorization_audit = extended;
    fixture.replace_first_stored_row(longer).await;
    fixture.assert_teardown_refuses_stored_rows().await;
}

/// After a dropped finalization, a stored row another writer changed stays a
/// difference although its audit is a prefix of the live one.
#[tokio::test]
async fn retry_after_a_dropped_finalization_does_not_excuse_a_changed_stored_row() {
    let (fixture, _) = MemoryFixture::cancelled_run_recording(1).await;
    fixture.drop_finalization_after_its_cas_commits().await;
    fixture.stage_refusal(&fixture.first_live_audit().await);
    let mut changed = fixture.stored_row(&fixture.inputs[0]).await;
    changed.state.updated_at += chrono::Duration::seconds(1);
    fixture.replace_first_stored_row(changed).await;
    fixture.assert_teardown_refuses_stored_rows().await;
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
                queued_inputs: mm::UnregisterQueuedInputsDisposition::Stop,
                requeue_max_age_ms: 0,
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
        owner::prepare_runtime_loop_batch_start(
            &driver,
            run_id.clone(),
            batch,
            crate::meerkat_machine::RunlessTerminalAnnouncement::default()
        )
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
