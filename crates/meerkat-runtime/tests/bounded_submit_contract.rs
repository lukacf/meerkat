#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! External-boundary contract for the bounded, typed dispatch surface.
//!
//! These run against the real `MeerkatMachine` (persistent and store-less), not
//! a stand-in, so the collapse assertions are backed by the generated admission
//! machine and the store-owned idempotency index rather than by a double.
//!
//! Runs are gated rather than raced. A test that lets the executor finish
//! whenever it likes cannot assert what the machine says about an input, and a
//! state assertion that depends on scheduling is worth less than no assertion
//! at all - it passes today and flips under load. Holding the gate pins the
//! phase, so "collapsed onto work that is still coming" and "collapsed onto
//! work that will never run" are two deterministic tests instead of one coin
//! flip.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use chrono::Utc;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::{RunResult, SessionId, Usage};
use meerkat_runtime::bounded_submit::{
    AdmissionDurability, AdmittedWorkState, BoundedSubmission, BoundedSubmitOutcome, SubmitBound,
    SubmitTimeoutCause, SubmitTimeoutDisposition, SubmitUnknownCause, submit_bounded,
};
use meerkat_runtime::identifiers::IdempotencyKey;
use meerkat_runtime::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PromptInput,
};
use meerkat_runtime::meerkat_machine::dsl::InputPublicTerminalOutcome;
use meerkat_runtime::store::{InMemoryRuntimeStore, RuntimeStore};
use meerkat_runtime::{MeerkatMachine, SessionServiceRuntimeExt};

/// A zero bound is the documented "do not wait at all" case, so every "the
/// caller's bound expired" assertion below rests on that contract rather than
/// on beating a fast admission to the answer. A bound merely *small* would be
/// a race, and a race asserted as a fact is a test that flips under load.
const EXPIRED: SubmitBound = SubmitBound::after(Duration::ZERO);

fn prompt(text: &str) -> Input {
    Input::Prompt(PromptInput {
        injected_context: Vec::new(),
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        content: text.into(),
        typed_turn_appends: Vec::new(),
        turn_metadata: None,
    })
}

fn run_result(text: &str) -> RunResult {
    RunResult {
        text: text.into(),
        session_id: SessionId::new(),
        usage: Usage::default(),
        turns: 1,
        tool_calls: 0,
        terminal_cause_kind: None,
        structured_output: None,
        extraction_error: None,
        schema_warnings: None,
        skill_diagnostics: None,
    }
}

/// Holds every run inside `apply` until the test releases it.
///
/// Closing the semaphore releases all current and future waiters permanently,
/// so a release can never be lost to arriving before the run that waits on it.
#[derive(Clone)]
struct RunGate(Arc<tokio::sync::Semaphore>);

impl RunGate {
    fn closed() -> Self {
        Self(Arc::new(tokio::sync::Semaphore::new(0)))
    }

    fn open() -> Self {
        let gate = Self::closed();
        gate.release();
        gate
    }

    fn release(&self) {
        self.0.close();
    }

    async fn wait(&self) {
        // Err is the released state; there are no permits to acquire.
        let _ = self.0.acquire().await;
    }
}

/// Records every delivery the runtime actually executes.
///
/// The collapse tests assert on this, not on the outcome tag: a report that
/// says `Collapsed` while the work ran twice is precisely the bug being pinned.
#[derive(Clone, Default)]
struct DeliveryLog(Arc<Mutex<Vec<Vec<InputId>>>>);

impl DeliveryLog {
    fn record(&self, input_ids: Vec<InputId>) {
        self.0
            .lock()
            .expect("delivery log poisoned")
            .push(input_ids);
    }

    fn deliveries(&self) -> Vec<Vec<InputId>> {
        self.0.lock().expect("delivery log poisoned").clone()
    }

    fn delivered(&self, input_id: &InputId) -> bool {
        self.deliveries()
            .iter()
            .any(|delivery| delivery.contains(input_id))
    }

    async fn wait_for(&self, want: usize) {
        for _ in 0..600 {
            if self.deliveries().len() >= want {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!(
            "expected {want} deliveries, observed {}",
            self.deliveries().len()
        );
    }
}

struct RecordingExecutor {
    log: DeliveryLog,
    gate: RunGate,
}

#[async_trait::async_trait]
impl CoreExecutor for RecordingExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let contributing = primitive.contributing_input_ids().to_vec();
        self.log.record(contributing.clone());
        self.gate.wait().await;
        Ok(CoreApplyOutput::with_run_result(
            RunBoundaryReceiptDraft {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: contributing,
                conversation_digest: None,
                message_count: 0,
            },
            None,
            run_result("delivered"),
        ))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

/// Let every detached admission and its run reach quiescence, so a second
/// delivery would have been observed if one were going to happen.
///
/// Callers release the gate first; an unreleased gate holds a run open forever
/// and this would spin out instead of settling.
async fn settle(machine: &Arc<MeerkatMachine>, session_id: &SessionId) {
    for _ in 0..600 {
        let active = machine
            .list_active_inputs(session_id)
            .await
            .expect("active inputs");
        if active.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
}

async fn persistent_session(log: &DeliveryLog, gate: &RunGate) -> (Arc<MeerkatMachine>, SessionId) {
    let store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let machine = Arc::new(MeerkatMachine::persistent_without_blobs(store));
    let session_id = SessionId::new();
    machine
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                log: log.clone(),
                gate: gate.clone(),
            }),
        )
        .await
        .expect("persistent runtime executor registration should succeed");
    (machine, session_id)
}

#[tokio::test]
async fn duplicate_submit_under_one_key_collapses_to_a_single_execution() {
    let log = DeliveryLog::default();
    let gate = RunGate::closed();
    let (machine, session_id) = persistent_session(&log, &gate).await;
    let key = IdempotencyKey::new("telegram-update-9001");

    let first = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("what is on my calendar"),
        BoundedSubmission::new(key.clone()),
    )
    .await;
    let admitted = match &first.outcome {
        BoundedSubmitOutcome::Admitted {
            input_id,
            durability: AdmissionDurability::Durable,
            state: AdmittedWorkState::Pending,
        } => input_id.clone(),
        other => panic!("expected a durable admission, got {other:?}"),
    };
    // The run is now held open inside `apply`, so the collapse target below is
    // pinned non-terminal rather than racing the executor to completion.
    log.wait_for(1).await;

    let second = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("what is on my calendar"),
        BoundedSubmission::new(key),
    )
    .await;
    match &second.outcome {
        BoundedSubmitOutcome::Collapsed {
            input_id,
            durability: AdmissionDurability::Durable,
            state: AdmittedWorkState::Pending,
        } => assert_eq!(input_id, &admitted),
        other => panic!("expected a durable collapse onto pending work, got {other:?}"),
    }
    assert!(second.is_durably_queued());

    gate.release();
    settle(&machine, &session_id).await;
    let deliveries = log.deliveries();
    assert_eq!(
        deliveries.len(),
        1,
        "one logical message must be delivered once, observed {deliveries:?}"
    );
    assert_eq!(
        first.admitted_input_id(),
        second.admitted_input_id(),
        "both callers must be told about the same admitted input"
    );
}

/// The adopter's regression: a delivery that succeeds but reports a timeout,
/// followed by the caller's retry. The retry must collapse, not deliver twice.
#[tokio::test]
async fn a_retry_after_an_expired_bound_does_not_double_deliver() {
    let log = DeliveryLog::default();
    let gate = RunGate::closed();
    let (machine, session_id) = persistent_session(&log, &gate).await;
    let key = IdempotencyKey::new("telegram-update-9002");

    let timed_out = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("book the table"),
        BoundedSubmission::new(key.clone()).with_bound(EXPIRED),
    )
    .await;
    assert!(
        matches!(
            timed_out.outcome,
            BoundedSubmitOutcome::TimedOut {
                cause: SubmitTimeoutCause::BoundExpired,
                ..
            }
        ),
        "a zero bound must expire, got {:?}",
        timed_out.outcome
    );

    let retry = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("book the table"),
        BoundedSubmission::new(key),
    )
    .await;
    // Which of the two racing submissions won the admission is deliberately not
    // asserted; that exactly one input exists and ran once is the contract.
    let retry_input = retry
        .admitted_input_id()
        .cloned()
        .unwrap_or_else(|| panic!("the retry must resolve to an admitted input: {retry:?}"));
    assert!(
        retry.is_durably_queued(),
        "the gate holds the run open, so the retry resolves onto durable pending work: {retry:?}"
    );

    log.wait_for(1).await;
    gate.release();
    settle(&machine, &session_id).await;

    let deliveries = log.deliveries();
    assert_eq!(
        deliveries.len(),
        1,
        "the retry must collapse, not deliver a second time; observed {deliveries:?}"
    );
    assert!(
        deliveries[0].contains(&retry_input),
        "the single delivery must be the input both callers were told about"
    );
}

#[tokio::test]
async fn an_expired_bound_reports_durably_queued_from_the_store_index() {
    let log = DeliveryLog::default();
    let gate = RunGate::closed();
    let (machine, session_id) = persistent_session(&log, &gate).await;
    let key = IdempotencyKey::new("telegram-update-9003");

    let admitted = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("remind me at six"),
        BoundedSubmission::new(key.clone()),
    )
    .await;
    let queued = admitted
        .admitted_input_id()
        .cloned()
        .expect("the first submission must admit an input");
    assert!(admitted.is_durably_queued());

    let timed_out = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("remind me at six"),
        BoundedSubmission::new(key).with_bound(EXPIRED),
    )
    .await;
    match &timed_out.outcome {
        BoundedSubmitOutcome::TimedOut {
            cause: SubmitTimeoutCause::BoundExpired,
            disposition:
                SubmitTimeoutDisposition::DurablyAdmitted {
                    input_id,
                    state: AdmittedWorkState::Pending,
                },
        } => assert_eq!(input_id, &queued),
        other => panic!("expected a durably-queued timeout, got {other:?}"),
    }
    assert!(timed_out.is_durably_queued());

    gate.release();
    settle(&machine, &session_id).await;
    assert_eq!(log.deliveries().len(), 1);
}

/// The field event, reproduced against the real machine: a key whose input was
/// terminalized without ever running. The durable row still exists and still
/// resolves the key, so the old "a row exists" test would call this queued
/// work and the host would wait for a reply that is never coming.
///
/// This is also the mutation check on the collapse branch: a collapse that
/// ignores the machine-owned seed cannot produce these assertions.
#[tokio::test]
async fn a_collapse_onto_work_that_will_never_run_is_not_reported_as_queued() {
    let log = DeliveryLog::default();
    let gate = RunGate::closed();
    let (machine, session_id) = persistent_session(&log, &gate).await;
    let cancelled_key = IdempotencyKey::new("telegram-update-9006");

    // Occupy the runtime so everything after this stays queued behind it.
    let occupying = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("the run that holds the loop"),
        BoundedSubmission::new(IdempotencyKey::new("telegram-update-9006-occupy")),
    )
    .await;
    let occupying_input = occupying
        .admitted_input_id()
        .cloned()
        .expect("the occupying submission must admit an input");
    log.wait_for(1).await;

    let doomed = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("cancel my six o'clock"),
        BoundedSubmission::new(cancelled_key.clone()),
    )
    .await;
    let doomed_input = doomed
        .admitted_input_id()
        .cloned()
        .expect("the doomed submission must admit an input");
    assert!(doomed.is_durably_queued(), "queued work starts out pending");

    // Ordinary cancellation, the first-class operation a host already has: the
    // queued input terminalizes without ever running, and its committed row
    // keeps resolving the key afterwards.
    let observed = machine
        .cancel_input_if_present(&session_id, &doomed_input, "operator cancelled the message")
        .await
        .expect("cancelling a queued input should succeed");
    assert!(observed, "the queued input must have been observed");

    let retry = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("cancel my six o'clock"),
        BoundedSubmission::new(cancelled_key),
    )
    .await;
    match &retry.outcome {
        BoundedSubmitOutcome::Collapsed {
            input_id,
            durability: AdmissionDurability::Durable,
            state:
                AdmittedWorkState::TerminalWithoutDelivery {
                    outcome: InputPublicTerminalOutcome::Cancelled,
                },
        } => assert_eq!(input_id, &doomed_input),
        other => panic!("expected a collapse onto cancelled work, got {other:?}"),
    }
    assert!(
        !retry.is_durably_queued(),
        "a durable row for work that will never run is not queued work: {retry:?}"
    );
    assert!(retry.is_terminal_without_delivery());

    gate.release();
    settle(&machine, &session_id).await;
    assert!(
        !log.delivered(&doomed_input),
        "the cancelled input must never have run, observed {:?}",
        log.deliveries()
    );
    assert!(log.delivered(&occupying_input));
}

#[tokio::test]
async fn a_store_less_runtime_never_claims_durable_queueing() {
    let log = DeliveryLog::default();
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    machine
        .register_session_with_executor(
            session_id.clone(),
            Box::new(RecordingExecutor {
                log: log.clone(),
                gate: RunGate::open(),
            }),
        )
        .await
        .expect("ephemeral runtime executor registration should succeed");

    let timed_out = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("nothing here survives a restart"),
        BoundedSubmission::new(IdempotencyKey::new("telegram-update-9004")).with_bound(EXPIRED),
    )
    .await;

    assert!(
        matches!(
            timed_out.outcome,
            BoundedSubmitOutcome::TimedOut {
                cause: SubmitTimeoutCause::BoundExpired,
                disposition: SubmitTimeoutDisposition::Unknown {
                    cause: SubmitUnknownCause::NoDurableWitness
                },
            }
        ),
        "a runtime that retains nothing must report unknown, got {:?}",
        timed_out.outcome
    );
    assert!(!timed_out.is_durably_queued());

    // The expired bound must not have cancelled the work it was only observing.
    log.wait_for(1).await;
    settle(&machine, &session_id).await;
    assert_eq!(log.deliveries().len(), 1);
}

#[tokio::test]
async fn a_caller_that_supplies_no_bound_gets_the_documented_default() {
    let submission = BoundedSubmission::new(IdempotencyKey::new("telegram-update-9005"));
    assert_eq!(submission.bound(), SubmitBound::default());
    assert_eq!(
        submission.bound().as_duration(),
        meerkat_runtime::bounded_submit::DEFAULT_SUBMIT_BOUND
    );

    let log = DeliveryLog::default();
    let gate = RunGate::open();
    let (machine, session_id) = persistent_session(&log, &gate).await;
    let report = submit_bounded(
        Arc::clone(&machine),
        &session_id,
        prompt("default bound"),
        submission,
    )
    .await;
    assert!(
        report.admitted_input_id().is_some(),
        "the default bound must admit rather than wait forever: {report:?}"
    );
    settle(&machine, &session_id).await;
}
