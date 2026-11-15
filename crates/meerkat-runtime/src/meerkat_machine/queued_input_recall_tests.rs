//! Recall of one selected queued input
//! ([`MeerkatMachine::recall_queued_input`]): the machine abandons the input
//! only while it is queued, a run that took it is never interrupted, and the
//! answer is typed (abandoned, not queued with its phase, already terminal).
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;

use crate::completion::CompletionOutcome;
use crate::input_state::{InputAbandonReason, InputLifecycleState, InputTerminalOutcome};
use crate::queued_input_recall::QueuedInputRecall;

const BOUND: Duration = Duration::from_secs(30);

fn cancelled() -> InputTerminalOutcome {
    InputTerminalOutcome::Abandoned {
        reason: InputAbandonReason::Cancelled,
    }
}

fn keyed_event(key: &str) -> Input {
    Input::ExternalEvent(crate::input::ExternalEventInput {
        objective_id: None,
        header: crate::input::InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::External {
                source_name: "queued-input-recall-test".into(),
            },
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: Some(crate::identifiers::IdempotencyKey::new(key)),
            supersession_key: None,
            correlation_id: None,
            requester: None,
        },
        event_type: "queued_input_recall".into(),
        payload: serde_json::json!({ "key": key }),
        blocks: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
    })
}

/// A durable operator prompt: the kind of queued input a graceful requeue
/// stop keeps for the next start.
fn keyed_prompt(key: &str) -> Input {
    Input::Prompt(crate::input::PromptInput {
        injected_context: Vec::new(),
        header: crate::input::InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Operator,
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: Some(crate::identifiers::IdempotencyKey::new(key)),
            supersession_key: None,
            correlation_id: None,
            requester: None,
        },
        content: "kept across a stop".into(),
        typed_turn_appends: Vec::new(),
        turn_metadata: None,
    })
}

fn persistent_machine_over(store: Arc<crate::store::InMemoryRuntimeStore>) -> Arc<MeerkatMachine> {
    Arc::new(
        MeerkatMachine::persistent(store, Arc::new(meerkat_store::MemoryBlobStore::new()))
            .expect("persistent machine"),
    )
}

fn persistent_machine() -> Arc<MeerkatMachine> {
    persistent_machine_over(Arc::new(crate::store::InMemoryRuntimeStore::new()))
}

async fn session_driver(
    machine: &MeerkatMachine,
    session_id: &SessionId,
) -> crate::meerkat_machine::driver::SharedDriver {
    machine
        .sessions
        .read()
        .await
        .get(session_id)
        .expect("registered session")
        .driver
        .clone()
}

/// Take the session mutation gate and give it back at once, so the accepted
/// boundary work of an admission (which holds the gate on the process-owned
/// runtime) is done before the test goes on.
async fn take_the_gate_once(machine: &MeerkatMachine, session_id: &SessionId) {
    drop(
        machine
            .session_mutation_gate(session_id)
            .await
            .expect("registered session gate")
            .lock_settled(SettledGateWait::TestFixture)
            .await,
    );
}

/// Accept one keyed event on a session no executor serves: it stays queued.
async fn queue(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    key: &str,
) -> (InputId, crate::completion::CompletionHandle) {
    queue_input(machine, session_id, keyed_event(key)).await
}

/// Accept `input` on a session no executor serves: it stays queued.
async fn queue_input(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input: Input,
) -> (InputId, crate::completion::CompletionHandle) {
    let input_id = input.id().clone();
    let (outcome, handle) = machine
        .accept_input_with_completion(session_id, input)
        .await
        .expect("accept the event");
    assert!(outcome.is_accepted());
    take_the_gate_once(machine, session_id).await;
    (
        input_id,
        handle.expect("an accepted input has a completion handle"),
    )
}

async fn phase(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input_id: &InputId,
) -> Option<InputLifecycleState> {
    session_driver(machine, session_id)
        .await
        .lock()
        .await
        .input_phase(input_id)
}

async fn recall(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input_id: &InputId,
) -> Option<QueuedInputRecall> {
    tokio::time::timeout(BOUND, machine.recall_queued_input(session_id, input_id))
        .await
        .expect("a recall never hangs")
        .expect("the recall answers")
}

async fn registered(machine: &Arc<MeerkatMachine>) -> SessionId {
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    session_id
}

// ---------------------------------------------------------------------------
// A queued input is abandoned, once
// ---------------------------------------------------------------------------

async fn a_queued_input_is_abandoned_and_stays_terminal(machine: Arc<MeerkatMachine>) {
    let session_id = registered(&machine).await;
    let (input_id, waiter) = queue(&machine, &session_id, "recall-queued").await;
    assert_eq!(
        phase(&machine, &session_id, &input_id).await,
        Some(InputLifecycleState::Queued)
    );

    assert_eq!(
        recall(&machine, &session_id, &input_id).await,
        Some(QueuedInputRecall::Abandoned {
            input_id: input_id.clone(),
            attempt_count: 0,
        })
    );

    // The row is terminal and cancelled, never bound to a run.
    let stored =
        <MeerkatMachine as SessionServiceRuntimeExt>::input_state(&machine, &session_id, &input_id)
            .await
            .expect("input state read")
            .expect("the recalled input's row is readable");
    assert_eq!(stored.seed.phase, InputLifecycleState::Abandoned);
    assert_eq!(stored.seed.terminal_outcome, Some(cancelled()));
    assert_eq!(stored.seed.last_run_id, None);
    assert!(
        <MeerkatMachine as SessionServiceRuntimeExt>::list_active_inputs(&machine, &session_id)
            .await
            .expect("active inputs")
            .is_empty(),
        "nothing is left for a run to take"
    );

    // The submitter's wait ends: the runtime terminated the input.
    let outcome = tokio::time::timeout(BOUND, waiter.try_wait())
        .await
        .expect("the waiter of a recalled input is resolved")
        .expect("the wait answers an outcome");
    assert!(
        matches!(outcome, CompletionOutcome::RuntimeTerminated { .. }),
        "a recalled input's waiter is told it was terminated before a run: {outcome:?}"
    );

    // Idempotent: the second recall changes nothing and says so.
    assert_eq!(
        recall(&machine, &session_id, &input_id).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: input_id.clone(),
            terminal: cancelled(),
        })
    );
}

#[tokio::test]
async fn ephemeral_recall_abandons_a_queued_input_and_a_second_recall_is_already_terminal() {
    a_queued_input_is_abandoned_and_stays_terminal(Arc::new(MeerkatMachine::ephemeral())).await;
}

#[tokio::test]
async fn persistent_recall_abandons_a_queued_input_and_a_second_recall_is_already_terminal() {
    a_queued_input_is_abandoned_and_stays_terminal(persistent_machine()).await;
}

/// K concurrent recalls of one queued input: exactly one abandons it, the
/// others answer already terminal, and all of them return.
async fn concurrent_recalls_abandon_once(machine: Arc<MeerkatMachine>) {
    const RECALLS: usize = 16;
    let session_id = registered(&machine).await;
    let (input_id, _waiter) = queue(&machine, &session_id, "recall-concurrent").await;

    let mut calls = Vec::with_capacity(RECALLS);
    for _ in 0..RECALLS {
        let machine = Arc::clone(&machine);
        let session_id = session_id.clone();
        let input_id = input_id.clone();
        calls.push(tokio::spawn(async move {
            machine.recall_queued_input(&session_id, &input_id).await
        }));
    }
    let (mut abandoned, mut already_terminal) = (0, 0);
    for call in calls {
        let answer = tokio::time::timeout(BOUND, call)
            .await
            .expect("no recall hangs")
            .expect("the recall task ends")
            .expect("the recall answers");
        match answer {
            Some(QueuedInputRecall::Abandoned { .. }) => abandoned += 1,
            Some(QueuedInputRecall::AlreadyTerminal { terminal, .. }) => {
                assert_eq!(terminal, cancelled());
                already_terminal += 1;
            }
            other => panic!("unexpected recall answer {other:?}"),
        }
    }
    assert_eq!(abandoned, 1, "exactly one recall abandons the input");
    assert_eq!(already_terminal, RECALLS - 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ephemeral_concurrent_recalls_abandon_the_input_exactly_once() {
    concurrent_recalls_abandon_once(Arc::new(MeerkatMachine::ephemeral())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persistent_concurrent_recalls_abandon_the_input_exactly_once() {
    concurrent_recalls_abandon_once(persistent_machine()).await;
}

// ---------------------------------------------------------------------------
// Unknown inputs and other sessions
// ---------------------------------------------------------------------------

/// An input of another session, and an input nobody admitted, are unknown
/// to this session: `None`, and the other session's input stays queued.
async fn another_sessions_input_is_unknown(machine: Arc<MeerkatMachine>) {
    let owner = registered(&machine).await;
    let other = registered(&machine).await;
    let (input_id, _waiter) = queue(&machine, &owner, "recall-other-session").await;

    assert_eq!(recall(&machine, &other, &input_id).await, None);
    assert_eq!(recall(&machine, &owner, &InputId::new()).await, None);
    assert_eq!(
        phase(&machine, &owner, &input_id).await,
        Some(InputLifecycleState::Queued),
        "the owner's input is untouched"
    );
}

#[tokio::test]
async fn ephemeral_recall_of_another_sessions_input_is_unknown() {
    another_sessions_input_is_unknown(Arc::new(MeerkatMachine::ephemeral())).await;
}

#[tokio::test]
async fn persistent_recall_of_another_sessions_input_is_unknown() {
    another_sessions_input_is_unknown(persistent_machine()).await;
}

// ---------------------------------------------------------------------------
// Started work is never interrupted
// ---------------------------------------------------------------------------

/// Executor whose every apply records its batch and waits for one permit.
struct GatedExecutor {
    session_id: SessionId,
    permits: Arc<crate::tokio::sync::Semaphore>,
    batches: Arc<std::sync::Mutex<Vec<(RunId, Vec<InputId>)>>>,
    calls: Arc<AtomicUsize>,
    cancels: Arc<AtomicUsize>,
}

fn run_result(session_id: &SessionId, text: &str) -> meerkat_core::RunResult {
    serde_json::from_value(serde_json::json!({
        "text": text,
        "session_id": session_id,
        "usage": meerkat_core::Usage::default(),
        "turns": 1,
        "tool_calls": 0,
    }))
    .expect("minimal RunResult")
}

#[async_trait::async_trait]
impl CoreExecutor for GatedExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let contributors = primitive.contributing_input_ids().to_vec();
        self.batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((run_id.clone(), contributors.clone()));
        self.permits
            .acquire()
            .await
            .map_err(|_| CoreExecutorError::Stopped)?
            .forget();
        Ok(CoreApplyOutput::with_run_result(
            RunBoundaryReceiptDraft {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: contributors,
                conversation_digest: None,
                message_count: 0,
            },
            None,
            run_result(&self.session_id, &format!("answer-{call}")),
        ))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

struct Served {
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
    permits: Arc<crate::tokio::sync::Semaphore>,
    batches: Arc<std::sync::Mutex<Vec<(RunId, Vec<InputId>)>>>,
    cancels: Arc<AtomicUsize>,
}

impl Served {
    async fn new(machine: Arc<MeerkatMachine>) -> Self {
        Self::serving(machine, SessionId::new()).await
    }

    /// Serve `session_id`: a new session, or one the machine's store already
    /// holds, which the registration recovers.
    async fn serving(machine: Arc<MeerkatMachine>, session_id: SessionId) -> Self {
        let permits = Arc::new(crate::tokio::sync::Semaphore::new(0));
        let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
        let cancels = Arc::new(AtomicUsize::new(0));
        machine
            .register_session_with_executor(
                session_id.clone(),
                Box::new(GatedExecutor {
                    session_id: session_id.clone(),
                    permits: Arc::clone(&permits),
                    batches: Arc::clone(&batches),
                    calls: Arc::new(AtomicUsize::new(0)),
                    cancels: Arc::clone(&cancels),
                }),
            )
            .await
            .expect("register gated executor");
        Self {
            machine,
            session_id,
            permits,
            batches,
            cancels,
        }
    }

    fn batches(&self) -> Vec<(RunId, Vec<InputId>)> {
        self.batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn ran(&self, input_id: &InputId) -> bool {
        self.batches()
            .iter()
            .any(|(_, contributors)| contributors.contains(input_id))
    }

    async fn wait_for_applies(&self, count: usize) {
        tokio::time::timeout(BOUND, async {
            while self.batches().len() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("executor apply starts");
    }

    async fn accept(&self, key: &str) -> (InputId, crate::completion::CompletionHandle) {
        let input = keyed_event(key);
        let input_id = input.id().clone();
        let (outcome, handle) = self
            .machine
            .accept_input_with_completion(&self.session_id, input)
            .await
            .expect("accept keyed event");
        assert!(outcome.is_accepted());
        (
            input_id,
            handle.expect("an accepted input has a completion handle"),
        )
    }

    async fn terminal(&self, input_id: &InputId) -> Option<InputTerminalOutcome> {
        <MeerkatMachine as SessionServiceRuntimeExt>::input_state(
            &self.machine,
            &self.session_id,
            input_id,
        )
        .await
        .expect("input state read")
        .and_then(|stored| stored.seed.terminal_outcome)
    }
}

/// An input a run has taken answers `NotQueued` with its phase and the run
/// that holds it. The run is not interrupted and completes; an input queued
/// behind it is recalled and never runs; the completed input then answers
/// `AlreadyTerminal`.
async fn a_started_input_is_left_to_its_run(machine: Arc<MeerkatMachine>) {
    let served = Served::new(machine).await;
    let (started, started_waiter) = served.accept("recall-started").await;
    served.wait_for_applies(1).await;
    let (run_id, batch) = served.batches().remove(0);
    assert_eq!(batch, vec![started.clone()]);
    let (behind, behind_waiter) = served.accept("recall-queued-behind").await;

    // The started input: nothing is touched.
    assert_eq!(
        recall(&served.machine, &served.session_id, &started).await,
        Some(QueuedInputRecall::NotQueued {
            input_id: started.clone(),
            phase: InputLifecycleState::Staged,
            run_id: Some(run_id.clone()),
        })
    );
    assert_eq!(
        phase(&served.machine, &served.session_id, &started).await,
        Some(InputLifecycleState::Staged)
    );
    assert_eq!(
        served.cancels.load(Ordering::SeqCst),
        0,
        "a recall never asks the executor to cancel"
    );

    // The input queued behind the run: withdrawn before the run ends.
    assert_eq!(
        recall(&served.machine, &served.session_id, &behind).await,
        Some(QueuedInputRecall::Abandoned {
            input_id: behind.clone(),
            attempt_count: 0,
        })
    );
    let behind_outcome = tokio::time::timeout(BOUND, behind_waiter.try_wait())
        .await
        .expect("the recalled input's waiter is resolved while the run still runs")
        .expect("the wait answers an outcome");
    assert!(
        matches!(behind_outcome, CompletionOutcome::RuntimeTerminated { .. }),
        "{behind_outcome:?}"
    );

    // The run completes with its own input, and only that one.
    served.permits.add_permits(1);
    let started_outcome = tokio::time::timeout(BOUND, started_waiter.try_wait())
        .await
        .expect("the started input completes")
        .expect("the wait answers an outcome");
    assert!(
        matches!(started_outcome, CompletionOutcome::Completed(_)),
        "the run was not interrupted: {started_outcome:?}"
    );
    // Give a wrongly surviving input the chance to run.
    served.permits.add_permits(4);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!served.ran(&behind), "a recalled input never reaches a run");
    assert_eq!(served.batches().len(), 1, "no second run started");
    assert_eq!(served.terminal(&behind).await, Some(cancelled()));

    assert_eq!(
        recall(&served.machine, &served.session_id, &started).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: started.clone(),
            terminal: InputTerminalOutcome::Consumed,
        })
    );
}

#[tokio::test]
async fn ephemeral_recall_leaves_a_started_input_to_its_run() {
    a_started_input_is_left_to_its_run(Arc::new(MeerkatMachine::ephemeral())).await;
}

#[tokio::test]
async fn persistent_recall_leaves_a_started_input_to_its_run() {
    a_started_input_is_left_to_its_run(persistent_machine()).await;
}

/// An input applied by a boundary (no lane, not terminal) is not queued
/// either: its phase is reported and nothing changes.
#[tokio::test]
async fn recall_of_an_applied_input_reports_its_phase() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = registered(&machine).await;
    let (input_id, _waiter) = queue(&machine, &session_id, "recall-applied").await;
    session_driver(&machine, &session_id)
        .await
        .lock()
        .await
        .apply_without_run_for_test(&input_id)
        .expect("step the input to applied without a run");

    assert_eq!(
        recall(&machine, &session_id, &input_id).await,
        Some(QueuedInputRecall::NotQueued {
            input_id: input_id.clone(),
            phase: InputLifecycleState::AppliedPendingConsumption,
            run_id: None,
        })
    );
    assert_eq!(
        phase(&machine, &session_id, &input_id).await,
        Some(InputLifecycleState::AppliedPendingConsumption)
    );
}

/// A recall racing the run that takes the input: exactly one wins. Either
/// the recall abandons the input and no run ever receives it, or a run took
/// it and the recall says so and leaves it alone.
async fn recall_races_the_run_that_takes_the_input(machine: Arc<MeerkatMachine>) {
    const ROUNDS: usize = 24;
    let served = Served::new(machine).await;
    // Every run completes at once.
    served.permits.add_permits(4 * ROUNDS);
    let (mut recalled, mut taken) = (0, 0);
    for round in 0..ROUNDS {
        let input = keyed_event(&format!("recall-race-{round}"));
        let input_id = input.id().clone();
        // The admission wakes the runtime loop, which stages the input; the
        // recall is issued at the same time.
        let accept = {
            let machine = Arc::clone(&served.machine);
            let session_id = served.session_id.clone();
            tokio::spawn(async move {
                machine
                    .accept_input_with_completion(&session_id, input)
                    .await
            })
        };
        let recall_call = {
            let machine = Arc::clone(&served.machine);
            let session_id = served.session_id.clone();
            let input_id = input_id.clone();
            tokio::spawn(async move {
                // Recall until the session holds the input: before the
                // admission commits it is the unknown input.
                loop {
                    match machine.recall_queued_input(&session_id, &input_id).await {
                        Ok(None) => tokio::task::yield_now().await,
                        other => return other,
                    }
                }
            })
        };
        let (outcome, waiter) = tokio::time::timeout(BOUND, accept)
            .await
            .expect("the admission returns")
            .expect("the admission task ends")
            .expect("the admission answers");
        assert!(outcome.is_accepted());
        let waiter = waiter.expect("completion handle");
        let answer = tokio::time::timeout(BOUND, recall_call)
            .await
            .expect("the racing recall never hangs")
            .expect("the recall task ends")
            .expect("the recall answers")
            .expect("the session holds the input");
        let outcome = tokio::time::timeout(BOUND, waiter.try_wait())
            .await
            .expect("the input reaches its terminal")
            .expect("the wait answers an outcome");
        match answer {
            QueuedInputRecall::Abandoned { attempt_count, .. } => {
                recalled += 1;
                assert_eq!(attempt_count, 0);
                assert!(
                    matches!(outcome, CompletionOutcome::RuntimeTerminated { .. }),
                    "round {round}: an abandoned input completes as terminated: {outcome:?}"
                );
                assert!(
                    !served.ran(&input_id),
                    "round {round}: the recall abandoned an input a run received"
                );
                assert_eq!(served.terminal(&input_id).await, Some(cancelled()));
            }
            QueuedInputRecall::NotQueued { .. } | QueuedInputRecall::AlreadyTerminal { .. } => {
                taken += 1;
                assert!(
                    matches!(outcome, CompletionOutcome::Completed(_)),
                    "round {round}: a run that took the input completes it: {outcome:?}"
                );
                assert!(
                    served.ran(&input_id),
                    "round {round}: the recall left the input alone, so a run received it"
                );
                assert_eq!(
                    served.terminal(&input_id).await,
                    Some(InputTerminalOutcome::Consumed)
                );
            }
        }
    }
    assert_eq!(recalled + taken, ROUNDS);
    // Which orders this run saw. Each order is pinned on its own by the
    // started-input rows; this row only needs every round to have one winner.
    eprintln!("recall racing a run start: the recall won {recalled} of {ROUNDS}, a run {taken}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ephemeral_recall_racing_a_run_start_has_exactly_one_winner() {
    recall_races_the_run_that_takes_the_input(Arc::new(MeerkatMachine::ephemeral())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn persistent_recall_racing_a_run_start_has_exactly_one_winner() {
    recall_races_the_run_that_takes_the_input(persistent_machine()).await;
}

// ---------------------------------------------------------------------------
// Across a stop and a resume
// ---------------------------------------------------------------------------

async fn stop_keeping_queued_inputs(machine: &MeerkatMachine, session_id: &SessionId) {
    let registration = machine
        .current_session_registration_witness(session_id)
        .await
        .expect("the session is registered");
    machine
        .unregister_session_registration_until_terminal_reporting_queued_inputs_if_current(
            &registration,
            UnregisterQueuedInputs::Requeue {
                max_age: Duration::from_mins(10),
            },
        )
        .await
        .expect("the requeue unregister completes");
    assert!(!machine.contains_session(session_id).await);
}

/// A recalled input stays terminal across a graceful stop that keeps queued
/// inputs and the next start: it is not recovered, and the recall still
/// answers already terminal, from the store while the session is stopped and
/// from the new registration afterwards.
#[tokio::test]
async fn a_recalled_input_stays_terminal_across_a_stop_and_a_resume() {
    let machine = persistent_machine();
    let session_id = registered(&machine).await;
    let (recalled, _waiter) =
        queue_input(&machine, &session_id, keyed_prompt("recall-then-stop")).await;
    let (kept, _kept_waiter) =
        queue_input(&machine, &session_id, keyed_prompt("kept-across-stop")).await;
    assert!(
        recall(&machine, &session_id, &recalled)
            .await
            .is_some_and(|answer| answer.abandoned())
    );

    stop_keeping_queued_inputs(&machine, &session_id).await;
    // Stopped: the terminal row answers from the store.
    assert_eq!(
        recall(&machine, &session_id, &recalled).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: recalled.clone(),
            terminal: cancelled(),
        })
    );
    // The kept input's phase lives in a machine that is not loaded: refused,
    // never loaded for the recall, and its row is untouched.
    let refused = machine.recall_queued_input(&session_id, &kept).await;
    assert!(
        matches!(
            refused,
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed
            })
        ),
        "a queued row of a session with no live registration is refused: {refused:?}"
    );
    assert!(!machine.contains_session(&session_id).await);

    machine
        .register_session(session_id.clone())
        .await
        .expect("the session starts again");
    assert_eq!(
        phase(&machine, &session_id, &recalled).await,
        None,
        "the recalled input is not recovered as work"
    );
    assert_eq!(
        recall(&machine, &session_id, &recalled).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: recalled.clone(),
            terminal: cancelled(),
        })
    );
    // The input the stop kept is queued again and can still be recalled.
    assert_eq!(
        phase(&machine, &session_id, &kept).await,
        Some(InputLifecycleState::Queued)
    );
    assert!(
        recall(&machine, &session_id, &kept)
            .await
            .is_some_and(|answer| answer.abandoned()),
        "an input kept across the stop is recalled once the session serves again"
    );
    assert_eq!(
        recall(&machine, &session_id, &kept).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: kept.clone(),
            terminal: cancelled(),
        })
    );
}

/// A session this machine never admitted is not found on a persistent
/// machine and not ready on a machine without a store, as the per-input
/// reads answer.
#[tokio::test]
async fn recall_on_a_session_without_a_registration_answers_as_the_input_reads_do() {
    let persistent = persistent_machine();
    let never_admitted = persistent
        .recall_queued_input(&SessionId::new(), &InputId::new())
        .await;
    assert!(
        matches!(never_admitted, Err(RuntimeDriverError::NotFound { .. })),
        "{never_admitted:?}"
    );
    let ephemeral = MeerkatMachine::ephemeral();
    let unregistered = ephemeral
        .recall_queued_input(&SessionId::new(), &InputId::new())
        .await;
    assert!(
        matches!(
            unregistered,
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed
            })
        ),
        "{unregistered:?}"
    );
}

// ---------------------------------------------------------------------------
// Recall is session-level
// ---------------------------------------------------------------------------

/// Peer progress in one supersession window: a later one coalesces the
/// queued earlier one into itself.
fn response_progress(label: &str) -> Input {
    Input::Peer(crate::input::PeerInput {
        directed_interaction_id: None,
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        header: crate::input::InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Peer {
                peer_id: "peer-1".into(),
                display_identity: None,
                runtime_id: None,
            },
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: Some(crate::identifiers::SupersessionKey::new("same-window")),
            correlation_id: None,
            requester: None,
        },
        convention: Some(crate::input::PeerConvention::ResponseProgress {
            request_id: format!("request-{label}"),
            phase: crate::input::ResponseProgressPhase::InProgress,
        }),
        content: format!("progress {label}").into(),
        payload: None,
        handling_mode: None,
        in_reply_to: None,
    })
}

/// A recall names one queued input of the session and withdraws it with
/// everything the queue coalesced into it. The earlier input is already
/// terminal (`Coalesced`, naming the input that took it in), so a recall of
/// it changes nothing; a recall of the input that took it in withdraws both
/// contents. The answer names only the recalled input: it does not say that
/// more than that input's own content was withdrawn.
async fn recall_withdraws_what_was_coalesced_into_the_input(machine: Arc<MeerkatMachine>) {
    let session_id = registered(&machine).await;
    let (earlier, _earlier_waiter) =
        queue_input(&machine, &session_id, response_progress("earlier")).await;
    let (aggregate, _aggregate_waiter) =
        queue_input(&machine, &session_id, response_progress("later")).await;
    let coalesced = InputTerminalOutcome::Coalesced {
        aggregate_id: aggregate.clone(),
    };

    // The earlier input ended when the later one took it in.
    assert_eq!(
        recall(&machine, &session_id, &earlier).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: earlier.clone(),
            terminal: coalesced.clone(),
        })
    );
    assert_eq!(
        phase(&machine, &session_id, &aggregate).await,
        Some(InputLifecycleState::Queued),
        "a recall of the coalesced input leaves the input that took it in queued"
    );

    // Recalling the input that took it in withdraws both contents.
    let answer = recall(&machine, &session_id, &aggregate)
        .await
        .expect("the session holds the input");
    assert_eq!(
        answer,
        QueuedInputRecall::Abandoned {
            input_id: aggregate.clone(),
            attempt_count: 0,
        }
    );
    assert!(
        <MeerkatMachine as SessionServiceRuntimeExt>::list_active_inputs(&machine, &session_id)
            .await
            .expect("active inputs")
            .is_empty(),
        "neither content is left for a run to take"
    );
    // The answer, as every surface reports it, names the recalled input
    // alone: nothing in it says that another input's content went with it.
    assert_eq!(
        serde_json::to_value(
            crate::queued_input_recall_wire::wire_queued_input_recall(&answer)
                .expect("the answer projects onto the wire")
        )
        .expect("the wire answer serializes"),
        serde_json::json!({
            "outcome": "abandoned",
            "input_id": aggregate.to_string(),
            "attempt_count": 0,
        })
    );
    // The earlier input keeps its own terminal: it is not turned into a
    // cancelled one.
    assert_eq!(
        recall(&machine, &session_id, &earlier).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: earlier.clone(),
            terminal: coalesced,
        })
    );
}

#[tokio::test]
async fn ephemeral_recall_withdraws_what_was_coalesced_into_the_input() {
    recall_withdraws_what_was_coalesced_into_the_input(Arc::new(MeerkatMachine::ephemeral())).await;
}

#[tokio::test]
async fn persistent_recall_withdraws_what_was_coalesced_into_the_input() {
    recall_withdraws_what_was_coalesced_into_the_input(persistent_machine()).await;
}

// ---------------------------------------------------------------------------
// Power cuts and failed commits
// ---------------------------------------------------------------------------

/// The process ends here, with the session still registered: nothing
/// unregisters, stops or retires it, so the store is all a later process
/// finds.
fn power_cut(machine: Arc<MeerkatMachine>) {
    drop(machine);
}

/// The store's own row of `input_id`.
async fn stored_row(
    store: &crate::store::InMemoryRuntimeStore,
    session_id: &SessionId,
    input_id: &InputId,
) -> Option<crate::input_state::StoredInputState> {
    store
        .load_input_state(&LogicalRuntimeId::for_session(session_id), input_id)
        .await
        .expect("read the input's stored row")
}

/// One recall, bounded, with its error kept.
async fn recall_result(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input_id: &InputId,
) -> Result<Option<QueuedInputRecall>, RuntimeDriverError> {
    tokio::time::timeout(BOUND, machine.recall_queued_input(session_id, input_id))
        .await
        .expect("a recall never hangs")
}

/// Fail the archival that follows the durable commit of the session's next
/// queued-input abandon: the abandon is in the store, its last step is not
/// done.
async fn fail_the_archive_of_the_next_queued_abandon(
    machine: &MeerkatMachine,
    session_id: &SessionId,
) {
    let driver = session_driver(machine, session_id).await;
    let mut guard = driver.lock().await;
    let crate::meerkat_machine::driver::DriverEntry::Persistent(persistent) = &mut *guard else {
        panic!("a persistent machine drives its sessions persistently");
    };
    persistent.force_queued_abandon_archive_failure_for_test = true;
}

/// Serve `session_id` on `machine` with a recording executor whose runs end
/// at once, and run one new event to its end. The new event is admitted
/// behind everything the session held queued, so by then a run has taken
/// every input the store kept as work.
async fn serve_until_a_new_input_completes(
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
) -> Served {
    let served = Served::serving(machine, session_id).await;
    served.permits.add_permits(64);
    let (_new_input, waiter) = served.accept("admitted-after-the-restart").await;
    let outcome = tokio::time::timeout(BOUND, waiter.try_wait())
        .await
        .expect("the new input reaches its terminal")
        .expect("the wait answers an outcome");
    assert!(
        matches!(outcome, CompletionOutcome::Completed(_)),
        "the restarted session runs new work: {outcome:?}"
    );
    served
}

/// An input recalled before a power cut is still recalled after it: the
/// next process does not recover it as work, no run takes it, and a recall
/// answers already terminal, from the store before the session serves again
/// and from the new registration afterwards. An input that was queued at the
/// cut is recovered and runs.
#[tokio::test]
async fn a_recalled_input_stays_recalled_across_a_power_cut() {
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    let machine = persistent_machine_over(Arc::clone(&store));
    let session_id = registered(&machine).await;
    let (recalled, _recalled_waiter) = queue_input(
        &machine,
        &session_id,
        keyed_prompt("recalled-before-the-cut"),
    )
    .await;
    let (queued, _queued_waiter) =
        queue_input(&machine, &session_id, keyed_prompt("queued-at-the-cut")).await;
    assert_eq!(
        recall(&machine, &session_id, &recalled).await,
        Some(QueuedInputRecall::Abandoned {
            input_id: recalled.clone(),
            attempt_count: 0,
        })
    );
    power_cut(machine);

    let restarted = persistent_machine_over(Arc::clone(&store));
    // Nothing serves the session yet: the store answers.
    assert_eq!(
        recall(&restarted, &session_id, &recalled).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: recalled.clone(),
            terminal: cancelled(),
        })
    );
    let served = serve_until_a_new_input_completes(restarted, session_id.clone()).await;
    assert!(
        served.ran(&queued),
        "the input that was queued at the cut is recovered and runs"
    );
    assert_eq!(
        served.terminal(&queued).await,
        Some(InputTerminalOutcome::Consumed)
    );
    assert!(
        !served.ran(&recalled),
        "no run takes an input recalled before the cut"
    );
    assert_eq!(
        phase(&served.machine, &session_id, &recalled).await,
        None,
        "the recalled input is not recovered as work"
    );
    assert_eq!(served.terminal(&recalled).await, Some(cancelled()));
    assert_eq!(
        recall(&served.machine, &session_id, &recalled).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: recalled.clone(),
            terminal: cancelled(),
        })
    );
}

/// Where a recall that was accepted but did not settle is cut.
#[derive(Clone, Copy, Debug)]
enum RecallCut {
    /// The store never took the abandon's commit.
    BeforeTheCommit,
    /// The store took the commit and its acknowledgement was lost.
    AfterTheCommitBeforeItsAcknowledgement,
    /// The store took the commit and the archival after it did not happen.
    AfterTheCommitBeforeTheArchive,
}

/// A recall cut before it settled is never answered "recalled", and after
/// the power cut it has settled exactly one way, the store's: either the
/// input is work again and a run takes it, or it is withdrawn and no run
/// ever does.
async fn a_recall_cut_before_it_settled_settles_one_way(cut: RecallCut) {
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    let machine = persistent_machine_over(Arc::clone(&store));
    let session_id = registered(&machine).await;
    let (input_id, _waiter) =
        queue_input(&machine, &session_id, keyed_prompt("recall-cut-short")).await;
    match cut {
        RecallCut::BeforeTheCommit => store.fail_next_machine_lifecycle_commit(),
        RecallCut::AfterTheCommitBeforeItsAcknowledgement => {
            store.lose_next_machine_lifecycle_commit_acknowledgement();
        }
        RecallCut::AfterTheCommitBeforeTheArchive => {
            fail_the_archive_of_the_next_queued_abandon(&machine, &session_id).await;
        }
    }
    let unsettled = recall_result(&machine, &session_id, &input_id).await;
    assert!(
        matches!(
            unsettled,
            Err(RuntimeDriverError::RecoveryRepairBlocked { .. })
        ),
        "{cut:?}: a recall that did not settle answers its failure, never an outcome: \
         {unsettled:?}"
    );
    power_cut(machine);

    let committed = !matches!(cut, RecallCut::BeforeTheCommit);
    let row = stored_row(&store, &session_id, &input_id)
        .await
        .expect("the input's row is stored");
    assert_eq!(
        row.seed.terminal_outcome,
        committed.then(cancelled),
        "{cut:?}: the store holds the abandon exactly when its commit landed"
    );

    let restarted = persistent_machine_over(Arc::clone(&store));
    let served = serve_until_a_new_input_completes(restarted, session_id.clone()).await;
    if committed {
        assert!(
            !served.ran(&input_id),
            "{cut:?}: no run takes an input whose recall committed"
        );
        assert_eq!(served.terminal(&input_id).await, Some(cancelled()));
        assert_eq!(
            recall(&served.machine, &session_id, &input_id).await,
            Some(QueuedInputRecall::AlreadyTerminal {
                input_id: input_id.clone(),
                terminal: cancelled(),
            })
        );
    } else {
        assert!(
            served.ran(&input_id),
            "{cut:?}: an input whose recall never committed is work again"
        );
        assert_eq!(
            served.terminal(&input_id).await,
            Some(InputTerminalOutcome::Consumed)
        );
        assert_eq!(
            recall(&served.machine, &session_id, &input_id).await,
            Some(QueuedInputRecall::AlreadyTerminal {
                input_id: input_id.clone(),
                terminal: InputTerminalOutcome::Consumed,
            })
        );
    }
}

#[tokio::test]
async fn a_recall_cut_before_its_commit_leaves_the_input_as_work_after_a_power_cut() {
    a_recall_cut_before_it_settled_settles_one_way(RecallCut::BeforeTheCommit).await;
}

#[tokio::test]
async fn a_recall_cut_after_its_commit_stays_recalled_after_a_power_cut() {
    a_recall_cut_before_it_settled_settles_one_way(
        RecallCut::AfterTheCommitBeforeItsAcknowledgement,
    )
    .await;
}

#[tokio::test]
async fn a_recall_cut_before_its_archive_stays_recalled_after_a_power_cut() {
    a_recall_cut_before_it_settled_settles_one_way(RecallCut::AfterTheCommitBeforeTheArchive).await;
}

/// The reload of a session a failed durable step degraded: the registration
/// is replaced by one loaded from the store.
async fn reload(
    machine: &MeerkatMachine,
    witness: &RuntimeSessionRegistrationWitness,
) -> ReloadRequiredRegistrationDisposition {
    tokio::time::timeout(
        BOUND,
        machine.recover_or_discard_reload_required_registration_if_current(witness),
    )
    .await
    .expect("the reload settles")
    .expect("the reload runs")
}

/// A recall whose durable commit fails, in a process that lives on. The
/// caller is answered the durability refusal, never an outcome. The store
/// holds the input queued, the session needs a reload, and until it has one
/// a recall is refused: it is not answered from the live machine, which
/// diverged from the store. The input's waiter is told nothing. After the
/// reload the input is queued again and a recall withdraws it, once.
#[tokio::test]
async fn a_recall_whose_commit_fails_answers_the_failure_and_the_input_stays_queued() {
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    let machine = persistent_machine_over(Arc::clone(&store));
    let session_id = registered(&machine).await;
    let witness = machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("the session is registered");
    let (input_id, waiter) =
        queue_input(&machine, &session_id, keyed_prompt("recall-commit-fails")).await;

    store.fail_next_machine_lifecycle_commit();
    let refused = recall_result(&machine, &session_id, &input_id).await;
    assert!(
        matches!(
            &refused,
            Err(RuntimeDriverError::RecoveryRepairBlocked { reason, .. })
                if reason.contains("abandon_queued_input_commit")
        ),
        "the failed commit is the recall's answer: {refused:?}"
    );
    let row = stored_row(&store, &session_id, &input_id)
        .await
        .expect("the input's row is stored");
    assert_eq!(row.seed.phase, InputLifecycleState::Queued);
    assert_eq!(row.seed.terminal_outcome, None);
    let required = machine
        .durability_reload_required(&session_id)
        .await
        .expect("the failed commit degraded the session");
    assert_eq!(required.operation, "abandon_queued_input_commit");

    let again = recall_result(&machine, &session_id, &input_id).await;
    assert!(
        matches!(again, Err(RuntimeDriverError::RecoveryRepairBlocked { .. })),
        "a degraded session refuses the recall; it answers no terminal the store does not \
         hold: {again:?}"
    );
    // The per-input read is not gated on durability: until the reload it
    // answers the live machine's row, which is ahead of the store (the
    // abandon was applied, its commit failed). A caller must not take that
    // read as the recall's answer; the reload below puts the store's row
    // back, queued.
    let degraded_read =
        <MeerkatMachine as SessionServiceRuntimeExt>::input_state(&machine, &session_id, &input_id)
            .await
            .expect("the per-input read answers on a degraded session")
            .expect("the live ledger holds the input");
    assert_eq!(
        degraded_read.seed.terminal_outcome,
        Some(cancelled()),
        "until the reload the read shows the live machine's row, not the store's"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(100), waiter.try_wait())
            .await
            .is_err(),
        "the waiter of an input that was not withdrawn is told nothing"
    );

    assert_eq!(
        reload(&machine, &witness).await,
        ReloadRequiredRegistrationDisposition::Discarded
    );
    assert_eq!(
        phase(&machine, &session_id, &input_id).await,
        Some(InputLifecycleState::Queued),
        "the reloaded session holds the input queued, as the store does"
    );
    assert_eq!(
        recall(&machine, &session_id, &input_id).await,
        Some(QueuedInputRecall::Abandoned {
            input_id: input_id.clone(),
            attempt_count: 0,
        })
    );
    assert_eq!(
        recall(&machine, &session_id, &input_id).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: input_id.clone(),
            terminal: cancelled(),
        })
    );
    let row = stored_row(&store, &session_id, &input_id)
        .await
        .expect("the input's row is stored");
    assert_eq!(row.seed.terminal_outcome, Some(cancelled()));
}

/// A recall whose abandon commits and whose archival then fails, in a
/// process that lives on. The caller is answered the durability refusal,
/// never an outcome, although the input is withdrawn: the store holds it
/// cancelled. The session needs a reload and refuses a recall until it has
/// one. After the reload the input is not recovered as work (nothing is
/// active) and a recall answers already terminal.
#[tokio::test]
async fn a_recall_whose_archive_fails_answers_the_failure_and_the_input_is_withdrawn() {
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    let machine = persistent_machine_over(Arc::clone(&store));
    let session_id = registered(&machine).await;
    let witness = machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("the session is registered");
    let (input_id, _waiter) =
        queue_input(&machine, &session_id, keyed_prompt("recall-archive-fails")).await;

    fail_the_archive_of_the_next_queued_abandon(&machine, &session_id).await;
    let refused = recall_result(&machine, &session_id, &input_id).await;
    assert!(
        matches!(
            &refused,
            Err(RuntimeDriverError::RecoveryRepairBlocked { reason, .. })
                if reason.contains("abandon_queued_input_archive")
        ),
        "the failed archival is the recall's answer: {refused:?}"
    );
    let row = stored_row(&store, &session_id, &input_id)
        .await
        .expect("the input's row is stored");
    assert_eq!(
        row.seed.terminal_outcome,
        Some(cancelled()),
        "the abandon committed before its archival failed"
    );
    let required = machine
        .durability_reload_required(&session_id)
        .await
        .expect("the failed archival degraded the session");
    assert_eq!(required.operation, "abandon_queued_input_archive");
    let again = recall_result(&machine, &session_id, &input_id).await;
    assert!(
        matches!(again, Err(RuntimeDriverError::RecoveryRepairBlocked { .. })),
        "a degraded session refuses the recall until its reload: {again:?}"
    );

    assert_eq!(
        reload(&machine, &witness).await,
        ReloadRequiredRegistrationDisposition::Discarded
    );
    // The reloaded session holds the input's terminal row until it is
    // archived, or no row once it is: never a phase a run can take.
    let reloaded_phase = phase(&machine, &session_id, &input_id).await;
    assert!(
        matches!(reloaded_phase, None | Some(InputLifecycleState::Abandoned)),
        "the withdrawn input is not recovered as work: {reloaded_phase:?}"
    );
    assert!(
        <MeerkatMachine as SessionServiceRuntimeExt>::list_active_inputs(&machine, &session_id)
            .await
            .expect("active inputs")
            .is_empty(),
        "nothing is left for a run to take"
    );
    assert_eq!(
        recall(&machine, &session_id, &input_id).await,
        Some(QueuedInputRecall::AlreadyTerminal {
            input_id: input_id.clone(),
            terminal: cancelled(),
        })
    );
}

// ---------------------------------------------------------------------------
// The internal cancel points act on the machine's answer
// ---------------------------------------------------------------------------
//
// Three cancel points read an input as queued and then ask the machine to
// abandon it, under one hold of the session's driver: the tracked-delivery
// cancel, the exact-input cancel and the archive's end of unresumed work.
// The read and the machine's guard are the same fact (`input_phases`), so in
// production the machine cannot answer "not queued" there. These rows stage
// that answer with a test-only step between the read and the abandon, and
// pin what each point then does: nothing of its own. It undoes the
// termination it had prepared and answers an error. No runtime termination
// is committed, published or told for an input the machine did not abandon,
// and the input is not reported as ended. On a persistent session the
// undone preparation is a fail-stop: the session needs a reload, which
// loads the store's rows, and the store never saw a termination.

/// Before the session's next queued-input abandon asks the machine, step its
/// input out of the queue (applied, with no run): the machine then answers
/// that the input is not queued.
async fn step_the_next_abandoned_input_out_of_the_queue(
    driver: &crate::meerkat_machine::driver::SharedDriver,
) {
    let mut guard = driver.lock().await;
    let crate::meerkat_machine::driver::DriverEntry::Persistent(persistent) = &mut *guard else {
        panic!("a persistent machine drives its sessions persistently");
    };
    persistent.step_input_out_of_queue_before_next_abandon_for_test = true;
}

/// Count the runless-terminal hints the machine tells its host: one per call
/// that committed a terminal without a run.
fn count_runless_terminal_tells(machine: &MeerkatMachine) -> Arc<AtomicUsize> {
    let tells = Arc::new(AtomicUsize::new(0));
    machine.set_runless_input_terminal_hint(Arc::new({
        let tells = Arc::clone(&tells);
        move || {
            tells.fetch_add(1, Ordering::SeqCst);
        }
    }));
    tells
}

/// A session of a persistent machine with one queued prompt under `key`,
/// whose next queued-input abandon the machine will answer "not queued".
struct NotQueuedAnswer {
    store: Arc<crate::store::InMemoryRuntimeStore>,
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
    input_id: InputId,
    waiter: crate::completion::CompletionHandle,
    tells: Arc<AtomicUsize>,
}

impl NotQueuedAnswer {
    async fn staged(key: &str) -> Self {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let machine = persistent_machine_over(Arc::clone(&store));
        let session_id = registered(&machine).await;
        let tells = count_runless_terminal_tells(&machine);
        let (input_id, waiter) = queue_input(&machine, &session_id, keyed_prompt(key)).await;
        Self {
            store,
            machine,
            session_id,
            input_id,
            waiter,
            tells,
        }
    }

    /// What the cancel point left: nothing of its own.
    async fn assert_nothing_was_cancelled(self) {
        assert_eq!(
            self.tells.load(Ordering::SeqCst),
            0,
            "the host is told of no terminal: none was committed"
        );
        assert_eq!(
            phase(&self.machine, &self.session_id, &self.input_id).await,
            Some(InputLifecycleState::AppliedPendingConsumption),
            "the input stays in the phase the machine holds it in"
        );
        let row = stored_row(&self.store, &self.session_id, &self.input_id)
            .await
            .expect("the input's row is stored");
        assert_eq!(
            row.seed.terminal_outcome, None,
            "the store holds no terminal for the input"
        );
        assert!(
            row.state.terminal_completion.is_none(),
            "the store holds no runtime termination for the input: {:?}",
            row.state.terminal_completion
        );
        let required = self
            .machine
            .durability_reload_required(&self.session_id)
            .await
            .expect("the undone preparation leaves the persistent session to its reload");
        assert_eq!(required.operation, "runless_terminal_preparation_rollback");
        let told = tokio::time::timeout(Duration::from_millis(100), self.waiter.try_wait()).await;
        assert!(
            !matches!(told, Ok(Ok(CompletionOutcome::RuntimeTerminated { .. }))),
            "the waiter of an input the machine did not abandon was told it was terminated"
        );
    }
}

/// The message of the error a cancel point answers when the machine
/// contradicts its queued read.
const NOT_QUEUED_ANSWER: &str = "answered that it is not queued";

/// Publish a member residency for the session, as a mob's host does before it
/// cancels a tracked input.
async fn resident_member(
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
) -> meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
    let member = meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
        mob_id: "queued-input-recall".to_string(),
        agent_identity: "worker".to_string(),
        host_id: "host-a".to_string(),
        binding_generation: 1,
        member_session_id: session_id.to_string(),
        generation: 0,
        fence_token: 1,
    };
    let residency = machine
        .begin_member_residency_update(session_id.clone())
        .await
        .commit(member.clone(), None)
        .await
        .expect("publish the member residency");
    drop(residency);
    member
}

/// The exact-input cancel (`cancel_input_if_present`): the machine answers
/// that the input it read as queued is not queued. The cancel answers that
/// as an error and cancels nothing.
#[tokio::test]
async fn the_exact_cancel_leaves_an_input_the_machine_says_is_not_queued() {
    let staged = NotQueuedAnswer::staged("exact-not-queued").await;
    let driver = session_driver(&staged.machine, &staged.session_id).await;
    step_the_next_abandoned_input_out_of_the_queue(&driver).await;

    let answer = tokio::time::timeout(
        BOUND,
        staged.machine.cancel_input_if_present(
            &staged.session_id,
            &staged.input_id,
            "cancelled while the machine says not queued",
        ),
    )
    .await
    .expect("the cancel answers");
    assert!(
        matches!(
            &answer,
            Err(RuntimeDriverError::Internal(reason)) if reason.contains(NOT_QUEUED_ANSWER)
        ),
        "the exact cancel answers the machine's contradiction: {answer:?}"
    );
    staged.assert_nothing_was_cancelled().await;
}

/// The tracked-delivery cancel
/// (`cancel_tracked_input_for_member_incarnation`): the machine answers that
/// the input it read as queued is not queued. The cancel answers that as an
/// error and cancels nothing.
#[tokio::test]
async fn the_tracked_cancel_leaves_an_input_the_machine_says_is_not_queued() {
    const KEY: &str = "tracked-not-queued";
    let staged = NotQueuedAnswer::staged(KEY).await;
    let member = resident_member(&staged.machine, &staged.session_id).await;
    let driver = session_driver(&staged.machine, &staged.session_id).await;
    step_the_next_abandoned_input_out_of_the_queue(&driver).await;

    let answer = tokio::time::timeout(
        BOUND,
        staged.machine.cancel_tracked_input_for_member_incarnation(
            &staged.session_id,
            KEY,
            &member,
        ),
    )
    .await
    .expect("the tracked cancel answers");
    assert!(
        matches!(
            &answer,
            Err(RuntimeDriverError::Internal(reason)) if reason.contains(NOT_QUEUED_ANSWER)
        ),
        "the tracked cancel answers the machine's contradiction: {answer:?}"
    );
    staged.assert_nothing_was_cancelled().await;
}

/// The archive's end of unresumed work
/// (`end_unresumed_work_with_archive_lease_before`): the machine answers
/// that an input the archive read as queued is not queued. The archive
/// answers that as an error, ends nothing for the input and does not report
/// it as ended.
#[tokio::test]
async fn the_archive_leaves_an_input_the_machine_says_is_not_queued() {
    let staged = NotQueuedAnswer::staged("archive-not-queued").await;
    let deadline = meerkat_core::time_compat::Instant::now() + BOUND;
    let lease = staged
        .machine
        .prepare_session_archive_lease_before(&staged.session_id, deadline)
        .await
        .expect("the archive's lease prepares")
        .expect("a registered session leaves the archive a runtime to retire");
    step_the_next_abandoned_input_out_of_the_queue(&lease.driver).await;

    let answer = staged
        .machine
        .end_unresumed_work_with_archive_lease_before(&lease, "session archived", deadline)
        .await;
    assert!(
        matches!(
            &answer,
            Err(RuntimeControlPlaneError::Internal(reason)) if reason.contains(NOT_QUEUED_ANSWER)
        ),
        "the archive answers the machine's contradiction and reports nothing as ended: \
         {answer:?}"
    );
    staged.assert_nothing_was_cancelled().await;
}

// ---------------------------------------------------------------------------
// The machine decides
// ---------------------------------------------------------------------------

/// The driver's answer is the machine's: each phase maps to its arm, the
/// no-op arms change nothing, and a run cannot take an input the machine
/// abandoned.
#[tokio::test]
async fn the_driver_reports_the_machines_arm_for_every_phase() {
    use crate::meerkat_machine::driver::QueuedInputAbandon;

    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = registered(&machine).await;
    let (queued, _w1) = queue(&machine, &session_id, "arm-queued").await;
    let (applied, _w2) = queue(&machine, &session_id, "arm-applied").await;
    let driver = session_driver(&machine, &session_id).await;
    let mut driver = driver.lock().await;
    driver
        .apply_without_run_for_test(&applied)
        .expect("step the input to applied without a run");

    // Preview answers without changing anything.
    assert_eq!(
        driver
            .preview_queued_input_abandon(&queued, InputAbandonReason::Cancelled)
            .expect("preview"),
        QueuedInputAbandon::Abandoned { attempt_count: 0 }
    );
    assert_eq!(
        driver.input_phase(&queued),
        Some(InputLifecycleState::Queued)
    );
    assert_eq!(
        driver
            .preview_queued_input_abandon(&applied, InputAbandonReason::Cancelled)
            .expect("preview"),
        QueuedInputAbandon::NotQueued {
            phase: InputLifecycleState::AppliedPendingConsumption
        }
    );
    assert_eq!(
        driver
            .preview_queued_input_abandon(&InputId::new(), InputAbandonReason::Cancelled)
            .expect("preview"),
        QueuedInputAbandon::Untracked
    );

    // The no-op arms through the applying path: false, and nothing moves.
    assert!(
        !driver
            .abandon_queued_input(&applied, InputAbandonReason::Cancelled)
            .await
            .expect("the not-queued arm answers")
    );
    assert_eq!(
        driver.input_phase(&applied),
        Some(InputLifecycleState::AppliedPendingConsumption)
    );
    assert!(
        !driver
            .abandon_queued_input(&InputId::new(), InputAbandonReason::Cancelled)
            .await
            .expect("the untracked arm answers")
    );

    // The queued arm abandons; afterwards the input is terminal for both the
    // preview and a second apply.
    assert!(
        driver
            .abandon_queued_input(&queued, InputAbandonReason::Cancelled)
            .await
            .expect("the queued arm abandons")
    );
    assert_eq!(
        driver
            .preview_queued_input_abandon(&queued, InputAbandonReason::Cancelled)
            .expect("preview"),
        QueuedInputAbandon::AlreadyTerminal {
            phase: InputLifecycleState::Abandoned
        }
    );
    assert!(
        !driver
            .abandon_queued_input(&queued, InputAbandonReason::Cancelled)
            .await
            .expect("the already-terminal arm answers")
    );
    assert_eq!(
        driver
            .as_driver()
            .stored_input_state(&queued)
            .and_then(|stored| stored.seed.terminal_outcome),
        Some(cancelled())
    );
}
