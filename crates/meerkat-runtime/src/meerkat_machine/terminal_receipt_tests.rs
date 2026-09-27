//! Terminal-receipt read and wait over the runtime's own input lifecycle.
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
use crate::input_state::InputTerminalOutcome;
use crate::terminal_status::{
    InputTerminalReceiptRead, InputTerminalReceiptScope, InputTerminalReceiptWait,
    InteractionSelector, Sourced, TerminalWitnessSource,
};

const BOUND: Duration = Duration::from_secs(30);

/// Executor whose every apply waits for one permit, records its batch, and
/// answers with a real `RunResult` (or a scripted retryable failure).
struct GatedResultExecutor {
    session_id: SessionId,
    permits: Arc<crate::tokio::sync::Semaphore>,
    batches: Arc<std::sync::Mutex<Vec<(RunId, Vec<InputId>)>>>,
    calls: Arc<AtomicUsize>,
    failing_calls: Vec<usize>,
}

/// Build the `RunResult` through serde so optional fields another change
/// adds to it keep their defaults here.
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
impl CoreExecutor for GatedResultExecutor {
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
        if self.failing_calls.contains(&call) {
            return Err(CoreExecutorError::apply_failed_unknown(format!(
                "scripted failure {call}"
            )));
        }
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
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

struct Harness {
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
    permits: Arc<crate::tokio::sync::Semaphore>,
    batches: Arc<std::sync::Mutex<Vec<(RunId, Vec<InputId>)>>>,
}

impl Harness {
    async fn new(machine: MeerkatMachine, failing_calls: Vec<usize>) -> Self {
        let machine = Arc::new(machine);
        let session_id = SessionId::new();
        let permits = Arc::new(crate::tokio::sync::Semaphore::new(0));
        let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
        machine
            .register_session_with_executor(
                session_id.clone(),
                Box::new(GatedResultExecutor {
                    session_id: session_id.clone(),
                    permits: Arc::clone(&permits),
                    batches: Arc::clone(&batches),
                    calls: Arc::new(AtomicUsize::new(0)),
                    failing_calls,
                }),
            )
            .await
            .expect("register gated executor");
        Self {
            machine,
            session_id,
            permits,
            batches,
        }
    }

    fn batches(&self) -> Vec<(RunId, Vec<InputId>)> {
        self.batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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

    async fn accept(&self, key: &str) -> InputId {
        let input = keyed_external_event(key);
        let input_id = input.id().clone();
        let outcome = <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(
            &self.machine,
            &self.session_id,
            input,
        )
        .await
        .expect("accept keyed external event");
        assert!(outcome.is_accepted());
        input_id
    }

    async fn read(&self, selector: InteractionSelector) -> Sourced<InputTerminalReceiptRead> {
        self.machine
            .input_terminal_receipt(&self.session_id, selector)
            .await
            .expect("terminal receipt read")
            .expect("input is known")
    }

    async fn waiter_count(&self) -> usize {
        let completions = {
            let sessions = self.machine.sessions.read().await;
            sessions
                .get(&self.session_id)
                .expect("registered session")
                .completions
                .clone()
        };
        completions.lock().await.debug_waiter_count()
    }

    async fn wait_resolved(&self, input_id: &InputId) -> Sourced<InputTerminalReceiptRead> {
        match tokio::time::timeout(
            BOUND,
            self.machine
                .wait_input_terminal_receipt(&self.session_id, input_id),
        )
        .await
        .expect("receipt wait resolves")
        .expect("receipt wait succeeds")
        {
            Some(InputTerminalReceiptWait::Resolved(read)) => read,
            other => panic!("expected a resolved receipt, got {other:?}"),
        }
    }
}

fn keyed_external_event(key: &str) -> Input {
    Input::ExternalEvent(crate::input::ExternalEventInput {
        objective_id: None,
        header: crate::input::InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::External {
                source_name: "terminal-receipt-test".into(),
            },
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: Some(crate::identifiers::IdempotencyKey::new(key)),
            supersession_key: None,
            correlation_id: None,
        },
        event_type: "terminal_receipt".into(),
        payload: serde_json::json!({ "key": key }),
        blocks: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
    })
}

fn finalized(read: &InputTerminalReceiptRead) -> &crate::terminal_status::InputTerminalReceipt {
    match read {
        InputTerminalReceiptRead::Finalized(receipt) => receipt,
        other => panic!("expected a finalized receipt, got {other:?}"),
    }
}

fn completed_text(outcome: &CompletionOutcome) -> &str {
    match outcome {
        CompletionOutcome::Completed(result) => &result.text,
        other => panic!("expected a completed outcome, got {other:?}"),
    }
}

fn sorted(mut ids: Vec<InputId>) -> Vec<InputId> {
    ids.sort_by(|a, b| a.0.cmp(&b.0));
    ids
}

/// Two queued inputs that one run consumes read the same run, the same
/// canonical recipient set and the same outcome, each under its own input id,
/// by both the id and the idempotency-key selector. A waiter armed before
/// the run stays pending while the run is gated, resolves after release, and
/// leaves no registration behind.
#[tokio::test]
async fn batched_inputs_share_one_run_receipt_and_waiters_resolve_after_release() {
    let harness = Harness::new(MeerkatMachine::ephemeral(), Vec::new()).await;
    let head = harness.accept("receipt-head").await;
    harness.wait_for_applies(1).await;
    let x = harness.accept("receipt-x").await;
    let y = harness.accept("receipt-y").await;

    let pending = harness.read(InteractionSelector::InputId(x.clone())).await;
    assert_eq!(pending.source, TerminalWitnessSource::LiveRuntime);
    match &pending.report {
        InputTerminalReceiptRead::Pending {
            input_id,
            terminal: None,
            ..
        } => assert_eq!(input_id, &x),
        other => panic!("queued input reads pending, got {other:?}"),
    }

    let baseline = harness.waiter_count().await;
    let machine = Arc::clone(&harness.machine);
    let session_id = harness.session_id.clone();
    let armed_x = x.clone();
    let waiter = tokio::spawn(async move {
        machine
            .wait_input_terminal_receipt(&session_id, &armed_x)
            .await
    });
    tokio::time::timeout(BOUND, async {
        while harness.waiter_count().await <= baseline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("waiter registers while the input is pending");
    assert!(!waiter.is_finished(), "a gated input must not resolve");

    // A dropped waiter unregisters itself.
    let dropped = tokio::time::timeout(
        Duration::from_millis(50),
        harness
            .machine
            .wait_input_terminal_receipt(&harness.session_id, &y),
    )
    .await;
    assert!(dropped.is_err(), "pending wait must not resolve");
    assert_eq!(harness.waiter_count().await, baseline + 1);

    harness.permits.add_permits(2);
    let resolved = match tokio::time::timeout(BOUND, waiter)
        .await
        .expect("armed waiter resolves")
        .expect("waiter task joins")
        .expect("waiter succeeds")
    {
        Some(InputTerminalReceiptWait::Resolved(read)) => read,
        other => panic!("expected resolved, got {other:?}"),
    };
    assert_eq!(resolved.source, TerminalWitnessSource::LiveRuntime);

    let batches = harness.batches();
    assert_eq!(batches.len(), 2, "head run plus one batched run");
    assert_eq!(batches[0].1, vec![head.clone()]);
    assert_eq!(
        sorted(batches[1].1.clone()),
        sorted(vec![x.clone(), y.clone()]),
        "queued external events fold into one run"
    );
    let batch_run = batches[1].0.clone();
    let recipients = sorted(vec![x.clone(), y.clone()]);

    for (input_id, key) in [(&x, "receipt-x"), (&y, "receipt-y")] {
        for selector in [
            InteractionSelector::InputId(input_id.clone()),
            InteractionSelector::IdempotencyKey(key.to_string()),
        ] {
            let read = harness.read(selector).await;
            let receipt = finalized(&read.report);
            assert_eq!(receipt.input_id(), input_id);
            assert_eq!(receipt.terminal(), &InputTerminalOutcome::Consumed);
            assert_eq!(
                receipt.scope(),
                &InputTerminalReceiptScope::Run {
                    run_id: batch_run.clone()
                }
            );
            assert_eq!(receipt.run_id(), Some(&batch_run));
            assert_eq!(receipt.recipient_input_ids(), recipients.as_slice());
            assert_eq!(receipt.owner_input_id(), &recipients[0]);
            assert_eq!(completed_text(receipt.outcome()), "answer-2");
            assert_eq!(receipt.attempt_count(), 1);
        }
    }
    let armed = finalized(&resolved.report);
    assert_eq!(armed.input_id(), &x);
    assert_eq!(armed.recipient_input_ids(), recipients.as_slice());

    let head_read = harness
        .read(InteractionSelector::InputId(head.clone()))
        .await;
    let head_receipt = finalized(&head_read.report);
    assert_eq!(
        head_receipt.recipient_input_ids(),
        std::slice::from_ref(&head)
    );
    assert_eq!(completed_text(head_receipt.outcome()), "answer-1");

    // A wait on an already-resolved input returns at once.
    let again = harness.wait_resolved(&y).await;
    assert_eq!(finalized(&again.report).run_id(), Some(&batch_run));
    assert_eq!(harness.waiter_count().await, baseline);
}

/// A failed attempt the machine requeues is not the input's terminal: the
/// waiter re-arms and resolves only with the later run, carrying that run's
/// id and the second attempt.
#[tokio::test]
async fn requeued_attempt_does_not_resolve_the_wait() {
    let harness = Harness::new(MeerkatMachine::ephemeral(), vec![1]).await;
    let x = harness.accept("retry-x").await;
    let machine = Arc::clone(&harness.machine);
    let session_id = harness.session_id.clone();
    let armed = x.clone();
    let waiter = tokio::spawn(async move {
        machine
            .wait_input_terminal_receipt(&session_id, &armed)
            .await
    });
    harness.wait_for_applies(1).await;
    harness.permits.add_permits(1);
    // The machine requeues the failed attempt and stages the retry, which
    // parks on the next permit: the input is still owed a terminal.
    harness.wait_for_applies(2).await;
    let retry = harness.read(InteractionSelector::InputId(x.clone())).await;
    assert!(
        matches!(
            retry.report,
            InputTerminalReceiptRead::Pending {
                terminal: None,
                attempt_count: 2,
                ..
            }
        ),
        "retry is pending on its second attempt: {retry:?}"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !waiter.is_finished(),
        "a requeued attempt must not resolve the terminal wait"
    );
    harness.permits.add_permits(1);
    let resolved = match tokio::time::timeout(BOUND, waiter)
        .await
        .expect("retry resolves the waiter")
        .expect("waiter joins")
        .expect("waiter succeeds")
    {
        Some(InputTerminalReceiptWait::Resolved(read)) => read,
        other => panic!("expected resolved, got {other:?}"),
    };
    let receipt = finalized(&resolved.report);
    let second_run = harness.batches()[1].0.clone();
    assert_eq!(receipt.run_id(), Some(&second_run));
    assert_eq!(receipt.attempt_count(), 2);
    assert_eq!(completed_text(receipt.outcome()), "answer-2");
}

/// Retiring a runtime with a queued input finalizes a runtime-termination
/// receipt: no run answered it.
#[tokio::test]
async fn retired_queued_input_reads_runtime_termination_receipt() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let input = keyed_external_event("retired-x");
    let x = input.id().clone();
    <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(&machine, &session_id, input)
        .await
        .expect("accept");
    <MeerkatMachine as SessionServiceRuntimeExt>::retire_runtime(&machine, &session_id)
        .await
        .expect("retire abandons the queued input");
    let read =
        match tokio::time::timeout(BOUND, machine.wait_input_terminal_receipt(&session_id, &x))
            .await
            .expect("retired receipt wait resolves")
            .expect("wait succeeds")
        {
            Some(InputTerminalReceiptWait::Resolved(read)) => read,
            other => panic!("expected resolved, got {other:?}"),
        };
    let receipt = finalized(&read.report);
    assert_eq!(
        receipt.scope(),
        &InputTerminalReceiptScope::RuntimeTermination
    );
    assert_eq!(receipt.run_id(), None);
    assert!(matches!(
        receipt.terminal(),
        InputTerminalOutcome::Abandoned { .. }
    ));
    assert!(
        matches!(
            receipt.outcome(),
            CompletionOutcome::RuntimeTerminated { .. }
        ),
        "runtime termination carries its typed outcome: {:?}",
        receipt.outcome()
    );
}

/// A coalesced input never gets a receipt; the reader types it instead of
/// reporting a lost receipt.
#[tokio::test]
async fn coalesced_input_reads_terminal_without_receipt() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let first = response_progress("first");
    let first_id = first.id().clone();
    <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(&machine, &session_id, first)
        .await
        .expect("accept first");
    let second = response_progress("second");
    let second_id = second.id().clone();
    <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(&machine, &session_id, second)
        .await
        .expect("accept second");

    let read = machine
        .input_terminal_receipt(&session_id, InteractionSelector::InputId(first_id.clone()))
        .await
        .expect("read")
        .expect("known input");
    match &read.report {
        InputTerminalReceiptRead::TerminalWithoutReceipt {
            input_id, terminal, ..
        } => {
            assert_eq!(input_id, &first_id);
            assert_eq!(
                terminal,
                &InputTerminalOutcome::Coalesced {
                    aggregate_id: second_id
                }
            );
        }
        other => panic!("expected a receipt-less terminal, got {other:?}"),
    }
    // The exact completion reader gives the same verdict for the same row
    // through its typed error: terminal, no receipt, no public completion.
    let legacy = <MeerkatMachine as SessionServiceRuntimeExt>::input_terminal_completion(
        &machine,
        &session_id,
        &first_id,
    )
    .await;
    match legacy {
        Err(RuntimeDriverError::InputTerminalWithoutReceipt { input_id, terminal }) => {
            assert_eq!(input_id, first_id);
            assert!(matches!(terminal, InputTerminalOutcome::Coalesced { .. }));
        }
        other => panic!("both readers classify a coalesced row alike, got {other:?}"),
    }
    let waited = tokio::time::timeout(
        BOUND,
        machine.wait_input_terminal_receipt(&session_id, &first_id),
    )
    .await
    .expect("receipt-less terminal wait returns")
    .expect("wait succeeds");
    assert!(matches!(
        waited,
        Some(InputTerminalReceiptWait::Resolved(Sourced {
            report: InputTerminalReceiptRead::TerminalWithoutReceipt { .. },
            ..
        }))
    ));
}

/// Response-progress peer input: a second one with the same supersession key
/// coalesces the first while it is still queued.
fn response_progress(label: &str) -> Input {
    Input::Peer(crate::input::PeerInput {
        directed_interaction_id: None,
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        header: crate::input::InputHeader {
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
        },
        convention: Some(crate::input::PeerConvention::ResponseProgress {
            request_id: format!("request-{label}"),
            phase: crate::input::ResponseProgressPhase::InProgress,
        }),
        content: format!("progress {label}").into(),
        payload: None,
        handling_mode: None,
    })
}

async fn session_completions(
    machine: &MeerkatMachine,
    session_id: &SessionId,
) -> SharedCompletionRegistry {
    let sessions = machine.sessions.read().await;
    sessions
        .get(session_id)
        .expect("registered session")
        .completions
        .clone()
}

async fn registered_waiter_count(machine: &MeerkatMachine, session_id: &SessionId) -> usize {
    session_completions(machine, session_id)
        .await
        .lock()
        .await
        .debug_waiter_count()
}

async fn registered_observer_count(machine: &MeerkatMachine, session_id: &SessionId) -> usize {
    session_completions(machine, session_id)
        .await
        .lock()
        .await
        .debug_receipt_less_observer_count()
}

/// Bound for "the wait returned because the transition woke it", well below
/// any caller bound, so a waiter that is never woken fails the test.
const PROMPT: Duration = Duration::from_secs(5);

/// Coalescing terminalizes a queued input inside a later admission and
/// stages no receipt. A waiter armed on that input before the admission is
/// woken at the admission's commit and returns the receipt-less terminal at
/// once instead of sleeping until its caller's bound.
#[tokio::test]
async fn waiter_armed_before_coalescing_returns_promptly_with_terminal_without_receipt() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let first = response_progress("first");
    let first_id = first.id().clone();
    <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(&machine, &session_id, first)
        .await
        .expect("accept first");

    let baseline = registered_waiter_count(&machine, &session_id).await;
    let waiter = {
        let machine = Arc::clone(&machine);
        let session_id = session_id.clone();
        let first_id = first_id.clone();
        tokio::spawn(async move {
            machine
                .wait_input_terminal_receipt(&session_id, &first_id)
                .await
        })
    };
    tokio::time::timeout(BOUND, async {
        while registered_waiter_count(&machine, &session_id).await <= baseline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("waiter arms while the input is queued");
    assert!(!waiter.is_finished(), "a queued input must not resolve");

    let second = response_progress("second");
    let second_id = second.id().clone();
    <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(&machine, &session_id, second)
        .await
        .expect("accept the coalescing input");

    let waited = tokio::time::timeout(PROMPT, waiter)
        .await
        .expect("coalescing wakes the armed waiter")
        .expect("waiter joins")
        .expect("wait succeeds");
    match waited {
        Some(InputTerminalReceiptWait::Resolved(Sourced {
            source: TerminalWitnessSource::LiveRuntime,
            report:
                InputTerminalReceiptRead::TerminalWithoutReceipt {
                    input_id, terminal, ..
                },
        })) => {
            assert_eq!(input_id, first_id);
            assert_eq!(
                terminal,
                InputTerminalOutcome::Coalesced {
                    aggregate_id: second_id
                }
            );
        }
        other => panic!("expected a live receipt-less terminal, got {other:?}"),
    }
    assert_eq!(
        registered_waiter_count(&machine, &session_id).await,
        baseline,
        "the woken waiter leaves no registration behind"
    );
    assert_eq!(
        registered_observer_count(&machine, &session_id).await,
        0,
        "the woken observer leaves no registration behind"
    );
}

/// Member-host boot revival silently cancels the interrupted predecessor's
/// inputs: `Abandoned { Cancelled }` with no receipt and no interaction
/// terminal. That is a machine transition, not a lost receipt, so the reader
/// types it, and a waiter armed before the revival returns it promptly.
#[tokio::test]
async fn boot_revival_abandonment_reads_terminal_without_receipt() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register predecessor session");
    let input = keyed_external_event("revived-x");
    let x = input.id().clone();
    <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(&machine, &session_id, input)
        .await
        .expect("queue the predecessor input");

    let baseline = registered_waiter_count(&machine, &session_id).await;
    let waiter = {
        let machine = Arc::clone(&machine);
        let session_id = session_id.clone();
        let x = x.clone();
        tokio::spawn(async move { machine.wait_input_terminal_receipt(&session_id, &x).await })
    };
    tokio::time::timeout(BOUND, async {
        while registered_waiter_count(&machine, &session_id).await <= baseline {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("waiter arms on the predecessor input");

    let permits = Arc::new(crate::tokio::sync::Semaphore::new(0));
    let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut pending = match machine
        .ensure_session_with_executor_factory(session_id.clone(), {
            let session_id = session_id.clone();
            move |_| {
                Box::new(GatedResultExecutor {
                    session_id: session_id.clone(),
                    permits: Arc::clone(&permits),
                    batches: Arc::clone(&batches),
                    calls: Arc::new(AtomicUsize::new(0)),
                    failing_calls: Vec::new(),
                }) as Box<dyn CoreExecutor>
            }
        })
        .await
        .expect("prepare the replacement attachment")
    {
        EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
        EnsureRuntimeExecutorAttachment::Existing(witness) => {
            panic!("replacement fixture unexpectedly found {witness:?}")
        }
    };
    let mut publication = pending
        .try_commit_with_retained_publication_lease_under_runtime_turn_finalization_boundary()
        .await
        .expect("retain the replacement publication boundary");
    assert_eq!(
        publication
            .abandon_recovered_predecessor_inputs()
            .await
            .expect("silently abandon the predecessor input"),
        1
    );

    let cancelled = InputTerminalOutcome::Abandoned {
        reason: crate::input_state::InputAbandonReason::Cancelled,
    };
    let waited = tokio::time::timeout(PROMPT, waiter)
        .await
        .expect("boot revival wakes the armed waiter")
        .expect("waiter joins")
        .expect("boot revival is not a lost receipt");
    match waited {
        Some(InputTerminalReceiptWait::Resolved(Sourced {
            report:
                InputTerminalReceiptRead::TerminalWithoutReceipt {
                    input_id, terminal, ..
                },
            ..
        })) => {
            assert_eq!(input_id, x);
            assert_eq!(terminal, cancelled);
        }
        other => panic!("expected a receipt-less terminal, got {other:?}"),
    }
    let read = machine
        .input_terminal_receipt(&session_id, InteractionSelector::InputId(x.clone()))
        .await
        .expect("boot revival abandonment reads")
        .expect("known input");
    assert!(
        matches!(
            &read.report,
            InputTerminalReceiptRead::TerminalWithoutReceipt { terminal, .. } if terminal == &cancelled
        ),
        "unexpected read {read:?}"
    );
    // The exact completion reader classifies the same row the same way.
    match <MeerkatMachine as SessionServiceRuntimeExt>::input_terminal_completion(
        &machine,
        &session_id,
        &x,
    )
    .await
    {
        Err(RuntimeDriverError::InputTerminalWithoutReceipt { input_id, terminal }) => {
            assert_eq!(input_id, x);
            assert_eq!(terminal, cancelled);
        }
        other => panic!("both readers classify boot revival alike, got {other:?}"),
    }

    let witness = publication
        .commit_with(|_| Ok(()))
        .await
        .expect("publish the replacement after predecessor abandonment");
    machine
        .unregister_executor_attachment_if_current(&witness)
        .await
        .expect("clean replacement attachment");
}

/// An unknown input reads the same through the read and the wait, whether
/// or not the session is registered: `Ok(None)` for a known session that
/// holds no such input, `NotReady` for an unregistered session on a
/// store-less machine, and `NotFound` for a never-admitted session on a
/// persistent machine. A pending durable row with no live registration
/// detaches instead of waiting.
#[tokio::test]
async fn unknown_input_reads_the_same_through_read_and_wait_with_or_without_registration() {
    async fn wait(
        machine: &MeerkatMachine,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<InputTerminalReceiptWait>, RuntimeDriverError> {
        tokio::time::timeout(
            BOUND,
            machine.wait_input_terminal_receipt(session_id, input_id),
        )
        .await
        .expect("a wait with nothing to wait on returns at once")
    }
    async fn read(
        machine: &MeerkatMachine,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<Sourced<InputTerminalReceiptRead>>, RuntimeDriverError> {
        machine
            .input_terminal_receipt(session_id, InteractionSelector::InputId(input_id.clone()))
            .await
    }

    // Store-less machine.
    let ephemeral = MeerkatMachine::ephemeral();
    let unregistered = SessionId::new();
    let unknown = InputId::new();
    assert!(matches!(
        read(&ephemeral, &unregistered, &unknown).await,
        Err(RuntimeDriverError::NotReady { .. })
    ));
    assert!(matches!(
        wait(&ephemeral, &unregistered, &unknown).await,
        Err(RuntimeDriverError::NotReady { .. })
    ));
    let session_id = SessionId::new();
    ephemeral
        .register_session(session_id.clone())
        .await
        .expect("register session");
    assert!(
        read(&ephemeral, &session_id, &unknown)
            .await
            .expect("read")
            .is_none()
    );
    assert!(
        wait(&ephemeral, &session_id, &unknown)
            .await
            .expect("wait")
            .is_none()
    );
    assert!(
        ephemeral
            .input_terminal_receipt(
                &session_id,
                InteractionSelector::IdempotencyKey("never-admitted".into())
            )
            .await
            .expect("key read")
            .is_none()
    );

    // Persistent machine: the same store read registered and unregistered.
    let store: Arc<dyn crate::store::RuntimeStore> =
        Arc::new(crate::store::InMemoryRuntimeStore::new());
    let attached = MeerkatMachine::persistent_without_blobs(Arc::clone(&store));
    let session_id = SessionId::new();
    attached
        .register_session(session_id.clone())
        .await
        .expect("register persistent session");
    let queued = keyed_external_event("queued-x");
    let queued_id = queued.id().clone();
    <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(&attached, &session_id, queued)
        .await
        .expect("queue a durable input");
    assert!(
        read(&attached, &session_id, &unknown)
            .await
            .expect("read")
            .is_none()
    );
    assert!(
        wait(&attached, &session_id, &unknown)
            .await
            .expect("wait")
            .is_none()
    );

    let detached = MeerkatMachine::persistent_without_blobs(Arc::clone(&store));
    assert!(
        read(&detached, &session_id, &unknown)
            .await
            .expect("read")
            .is_none()
    );
    assert!(
        wait(&detached, &session_id, &unknown)
            .await
            .expect("wait")
            .is_none()
    );
    let never_admitted = SessionId::new();
    assert!(matches!(
        read(&detached, &never_admitted, &unknown).await,
        Err(RuntimeDriverError::NotFound { .. })
    ));
    assert!(matches!(
        wait(&detached, &never_admitted, &unknown).await,
        Err(RuntimeDriverError::NotFound { .. })
    ));
    match wait(&detached, &session_id, &queued_id).await {
        Ok(Some(InputTerminalReceiptWait::Detached(Sourced {
            source: TerminalWitnessSource::DurableStore,
            report: InputTerminalReceiptRead::Pending { input_id, .. },
        }))) => assert_eq!(input_id, queued_id),
        other => panic!("a pending row with no live registration detaches, got {other:?}"),
    }
}

/// A finalized receipt survives restart: a fresh machine over the same store
/// reads it by key without registering the session, and it equals the read
/// the attached machine made before the restart.
#[cfg(all(not(target_arch = "wasm32"), feature = "sqlite-store"))]
#[tokio::test]
async fn finalized_receipt_is_read_from_the_store_after_restart() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("runtime.sqlite3");
    let store = Arc::new(crate::store::SqliteRuntimeStore::new(path.clone()).expect("store"))
        as Arc<dyn crate::store::RuntimeStore>;
    let harness = Harness::new(
        MeerkatMachine::persistent(
            Arc::clone(&store),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ),
        Vec::new(),
    )
    .await;
    let x = harness.accept("restart-x").await;
    harness.permits.add_permits(1);
    let live = harness.wait_resolved(&x).await;
    let live_receipt = finalized(&live.report).clone();
    assert_eq!(completed_text(live_receipt.outcome()), "answer-1");
    let session_id = harness.session_id.clone();
    drop(harness);
    drop(store);

    let restarted_store = Arc::new(crate::store::SqliteRuntimeStore::new(path).expect("reopen"))
        as Arc<dyn crate::store::RuntimeStore>;
    let restarted = MeerkatMachine::persistent(
        restarted_store,
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    );
    let read = restarted
        .input_terminal_receipt(
            &session_id,
            InteractionSelector::IdempotencyKey("restart-x".into()),
        )
        .await
        .expect("durable read")
        .expect("durable input");
    assert_eq!(read.source, TerminalWitnessSource::DurableStore);
    let durable = finalized(&read.report);
    assert_eq!(durable.input_id(), &x);
    assert_eq!(durable.run_id(), live_receipt.run_id());
    assert_eq!(
        durable.recipient_input_ids(),
        live_receipt.recipient_input_ids()
    );
    assert_eq!(durable.terminal(), live_receipt.terminal());
    assert_eq!(completed_text(durable.outcome()), "answer-1");

    let waited = restarted
        .wait_input_terminal_receipt(&session_id, &x)
        .await
        .expect("unregistered wait");
    match waited {
        Some(InputTerminalReceiptWait::Resolved(read)) => {
            assert_eq!(read.source, TerminalWitnessSource::DurableStore);
            assert_eq!(finalized(&read.report).run_id(), live_receipt.run_id());
        }
        other => panic!("expected a resolved durable receipt, got {other:?}"),
    }
    assert!(
        restarted
            .input_terminal_receipt(
                &session_id,
                InteractionSelector::IdempotencyKey("restart-unknown".into())
            )
            .await
            .expect("unknown key read")
            .is_none()
    );
}
