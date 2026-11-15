//! What becomes of work queued on a session when one of its turns fails at
//! its start and the session's runtime is then retired, as the completion
//! relay of a surface retires it. The rows measure today's answers and pin
//! them: each expected value below was read from a run, none is a wish.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::*;
use std::time::Duration;

use chrono::Utc;
use meerkat_core::lifecycle::core_executor::{
    CoreApplyFailureCause, CoreApplyFailureCauseKind, CoreApplyOutput, CoreExecutor,
    CoreExecutorError,
};
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::run_primitive::RunPrimitive;
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;

use crate::completion::{CompletionHandle, CompletionOutcome};

const BOUND: Duration = Duration::from_secs(30);

fn header(key: &str, source: crate::input::InputOrigin) -> crate::input::InputHeader {
    crate::input::InputHeader {
        ingress_context: None,
        retained_resume: None,
        authority_association: None,
        id: InputId::new(),
        timestamp: Utc::now(),
        source,
        durability: crate::input::InputDurability::Durable,
        visibility: crate::input::InputVisibility::default(),
        idempotency_key: Some(crate::identifiers::IdempotencyKey::new(key)),
        supersession_key: None,
        correlation_id: None,
        requester: None,
    }
}

/// A durable external event in the queue lane.
fn event(key: &str) -> Input {
    Input::ExternalEvent(crate::input::ExternalEventInput {
        objective_id: None,
        header: header(
            key,
            crate::input::InputOrigin::External {
                source_name: "queued-work-row".into(),
            },
        ),
        event_type: "queued_work".into(),
        payload: serde_json::json!({ "key": key }),
        blocks: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata: None,
    })
}

/// A durable operator prompt: the kind a graceful requeue stop keeps.
fn prompt(key: &str) -> Input {
    Input::Prompt(crate::input::PromptInput {
        injected_context: Vec::new(),
        header: header(key, crate::input::InputOrigin::Operator),
        content: "queued behind a failed start".into(),
        typed_turn_appends: Vec::new(),
        turn_metadata: None,
    })
}

/// An inbound peer request in the queue lane, as the comms drain hands it to
/// the runtime: the request's id is its interaction's.
fn peer_request(
    key: &str,
    request: meerkat_core::PeerCorrelationId,
    requester: meerkat_core::comms::PeerId,
) -> Input {
    Input::Peer(crate::input::PeerInput {
        directed_interaction_id: None,
        objective_id: None,
        system_prompts: Vec::new(),
        injected_context: Vec::new(),
        sender_taint: None,
        header: header(
            key,
            crate::input::InputOrigin::Peer {
                peer_id: requester.0.to_string(),
                display_identity: Some("peer".into()),
                runtime_id: Some(LogicalRuntimeId::new("rt:peer:0")),
            },
        ),
        convention: Some(crate::input::PeerConvention::Request {
            request_id: request.as_uuid().to_string(),
            intent: "queued_work".into(),
        }),
        content: meerkat_core::types::ContentInput::Blocks(vec![
            meerkat_core::types::ContentBlock::Text {
                text: "Answer when you get to it".into(),
            },
        ]),
        payload: Some(serde_json::json!({ "key": key })),
        handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
        in_reply_to: None,
    })
}

/// Executor whose every apply waits for one permit, and whose first
/// `failing` applies then fail as a turn that could not be started. A closed
/// gate ends an apply as stopped.
struct StartFailingExecutor {
    session_id: SessionId,
    gate: Arc<crate::tokio::sync::Semaphore>,
    failing: usize,
    batches: Arc<std::sync::Mutex<Vec<Vec<InputId>>>>,
}

#[async_trait::async_trait]
impl CoreExecutor for StartFailingExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let contributors = primitive.contributing_input_ids().to_vec();
        let call = {
            let mut batches = self
                .batches
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            batches.push(contributors.clone());
            batches.len()
        };
        self.gate
            .acquire()
            .await
            .map_err(|_| CoreExecutorError::Stopped)?
            .forget();
        if call <= self.failing {
            return Err(CoreExecutorError::ApplyFailed {
                cause: CoreApplyFailureCause::new(
                    CoreApplyFailureCauseKind::RuntimeTurn,
                    "the turn could not be started",
                ),
            });
        }
        let result: meerkat_core::RunResult = serde_json::from_value(serde_json::json!({
            "text": format!("answer-{call}"),
            "session_id": self.session_id,
            "usage": meerkat_core::Usage::default(),
            "turns": 1,
            "tool_calls": 0,
        }))
        .expect("minimal RunResult");
        Ok(CoreApplyOutput::with_run_result(
            RunBoundaryReceiptDraft {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: contributors,
                conversation_digest: None,
                message_count: 0,
            },
            None,
            result,
        ))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

/// One registration of a session on `machine`, served by a
/// [`StartFailingExecutor`].
struct Registration {
    gate: Arc<crate::tokio::sync::Semaphore>,
    batches: Arc<std::sync::Mutex<Vec<Vec<InputId>>>>,
}

impl Registration {
    async fn of(machine: &Arc<MeerkatMachine>, session_id: &SessionId, failing: usize) -> Self {
        let gate = Arc::new(crate::tokio::sync::Semaphore::new(0));
        let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
        machine
            .register_session_with_executor(
                session_id.clone(),
                Box::new(StartFailingExecutor {
                    session_id: session_id.clone(),
                    gate: Arc::clone(&gate),
                    failing,
                    batches: Arc::clone(&batches),
                }),
            )
            .await
            .expect("register the session with its executor");
        Self { gate, batches }
    }

    fn applies(&self) -> usize {
        self.batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn took(&self, input_id: &InputId) -> bool {
        self.batches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|batch| batch.contains(input_id))
    }

    /// Whether the executor's `count`th apply started within `within`.
    async fn apply_started(&self, count: usize, within: Duration) -> bool {
        tokio::time::timeout(within, async {
            while self.applies() < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .is_ok()
    }
}

fn persistent_machine() -> Arc<MeerkatMachine> {
    Arc::new(
        MeerkatMachine::persistent(
            Arc::new(crate::store::InMemoryRuntimeStore::new()),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("persistent machine"),
    )
}

async fn accept(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input: Input,
) -> (InputId, CompletionHandle) {
    let input_id = input.id().clone();
    let (outcome, handle) = machine
        .accept_input_with_completion(session_id, input)
        .await
        .expect("accept the input");
    assert!(outcome.is_accepted());
    (
        input_id,
        handle.expect("an accepted input has a completion handle"),
    )
}

/// What a waiter was answered: the outcome's name, or the wait failure.
async fn answer(handle: CompletionHandle) -> String {
    match tokio::time::timeout(Duration::from_secs(5), handle.try_wait()).await {
        Err(_) => "not answered".to_string(),
        Ok(Err(error)) => format!("wait failure: {error}"),
        Ok(Ok(CompletionOutcome::Completed(_))) => "Completed".to_string(),
        Ok(Ok(CompletionOutcome::RuntimeTerminated { .. })) => "RuntimeTerminated".to_string(),
        Ok(Ok(other)) => {
            let text = format!("{other:?}");
            text.split(|c: char| !c.is_alphanumeric())
                .next()
                .unwrap_or_default()
                .to_string()
        }
    }
}

/// An input's stored record: phase, terminal, attempts counted, whether a
/// run is on record for it, and whether a stop kept it for the next start.
async fn stored(
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    input_id: &InputId,
) -> String {
    match <MeerkatMachine as SessionServiceRuntimeExt>::input_state(machine, session_id, input_id)
        .await
    {
        Err(error) => format!("read failed: {error}"),
        Ok(None) => "no row".to_string(),
        Ok(Some(stored)) => format!(
            "{:?}, terminal {:?}, attempts {}, run on record {}, kept for the next start {}",
            stored.seed.phase,
            stored.seed.terminal_outcome,
            stored.seed.attempt_count,
            stored.seed.last_run_id.is_some(),
            stored.state.stop_requeue.is_some()
        ),
    }
}

/// Retire the session's runtime as a surface's completion cleanup does: the
/// unregister of the current registration, joined until it is terminal. A
/// run the retirement waits for is ended as stopped after a moment.
async fn retire(
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    registration: &Registration,
) -> String {
    let Some(witness) = machine
        .current_session_registration_witness(session_id)
        .await
    else {
        return "no registration".to_string();
    };
    let retiring = {
        let machine = Arc::clone(machine);
        tokio::spawn(async move {
            machine
                .unregister_session_registration_until_terminal_if_current(&witness)
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    registration.gate.close();
    match tokio::time::timeout(BOUND, retiring).await {
        Err(_) => "the retirement did not return".to_string(),
        Ok(joined) => match joined.expect("the retirement does not panic") {
            Ok(done) => format!("retired {done}"),
            Err(error) => format!("retirement failed: {error}"),
        },
    }
}

/// Whether the machine's authority for the session records the inbound peer
/// request `request`.
async fn inbound_request_on_record(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    request: meerkat_core::PeerCorrelationId,
) -> String {
    let driver = match machine.sessions.read().await.get(session_id) {
        Some(entry) => entry.driver.clone(),
        None => return "no registration".to_string(),
    };
    let authority = {
        let driver = driver.lock().await;
        driver.shared_dsl_authority()
    };
    let authority = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let held = &authority.state().inbound_peer_requests;
    format!(
        "{} (inbound requests held: {})",
        format!("{held:?}").contains(&request.as_uuid().to_string()),
        held.len()
    )
}

/// What became of an input the retirement ended before any run took it: its
/// waiter is answered that the runtime terminated, its stored record is
/// abandoned as stopped with no attempt counted and no run on record, and
/// nothing is kept for the session's next start.
const NEVER_STAGED: &str = "taken by a run false, answered RuntimeTerminated, stored Abandoned, \
     terminal Some(Abandoned { reason: Stopped }), attempts 0, run on record false, kept for \
     the next start false";

/// What became of an input a run had taken when the retirement ended it:
/// as [`NEVER_STAGED`], with one attempt counted and its run on record.
const STAGED_ONCE: &str = "taken by a run true, answered RuntimeTerminated, stored Abandoned, \
     terminal Some(Abandoned { reason: Stopped }), attempts 1, run on record true, kept for \
     the next start false";

/// The answer of the turn that failed at its start.
const FAILED: &str = "AbandonedWithError";

/// One input's end, in the form of [`NEVER_STAGED`].
async fn ended(
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
    registration: &Registration,
    input_id: &InputId,
    waiter: CompletionHandle,
) -> String {
    format!(
        "taken by a run {}, answered {}, stored {}",
        registration.took(input_id),
        answer(waiter).await,
        stored(machine, session_id, input_id).await
    )
}

/// Whether the loop took the input into a run before the retirement ended
/// it is a race the rows do not decide: the input's end is one of the two.
fn staged_or_not(end: &str) -> bool {
    end == NEVER_STAGED || end == STAGED_ONCE
}

/// A turn fails at its start with two inputs queued behind it (a prompt,
/// then an external event), and the runtime is retired for it.
/// `after_the_next` waits, before the retirement, until the loop has taken
/// the next input into a run; otherwise the retirement follows the failed
/// turn's answer at once, as a relay's does. Returns (the failed turn's
/// answer, whether the loop had taken the next input when the retirement
/// was asked, the retirement, the first input's end, the second's).
async fn behind_a_failed_start(after_the_next: bool) -> (String, bool, String, String, String) {
    let machine = persistent_machine();
    let session_id = SessionId::new();
    let registration = Registration::of(&machine, &session_id, 1).await;
    let (_failing, failing_waiter) = accept(&machine, &session_id, event("fails")).await;
    assert!(registration.apply_started(1, BOUND).await);
    let (first, first_waiter) = accept(&machine, &session_id, prompt("behind-1")).await;
    let (second, second_waiter) = accept(&machine, &session_id, event("behind-2")).await;
    registration.gate.add_permits(1);
    let failed = answer(failing_waiter).await;
    let next_taken = if after_the_next {
        registration.apply_started(2, Duration::from_secs(5)).await
    } else {
        registration.applies() >= 2
    };
    let retired = retire(&machine, &session_id, &registration).await;
    (
        failed,
        next_taken,
        retired,
        ended(&machine, &session_id, &registration, &first, first_waiter).await,
        ended(&machine, &session_id, &registration, &second, second_waiter).await,
    )
}

/// At the retirement that follows a failed start, the inputs behind the
/// failed turn are NOT all still unstaged: the failure does not close
/// staging. The loop goes on to the next input, and if it takes it before
/// the retirement is asked, that input ends with one attempt counted and a
/// run on record. With the retirement asked at once after the failed
/// answer, which of the two happens is a race (the first input behind was
/// staged in 5 of 12 runs, and in 1 of 12, when this row was written).
/// Either way every input behind the failed
/// turn ends at the retirement, abandoned as stopped, its waiter answered
/// that the runtime terminated, and nothing is kept for the next start.
#[tokio::test]
async fn the_inputs_behind_a_failed_start_at_the_retirement() {
    // The loop got to its next take before the retirement: the first input
    // behind the failed turn is staged, the one behind it is not.
    let (failed, next_taken, retired, first, second) = behind_a_failed_start(true).await;
    assert_eq!(
        (
            failed.as_str(),
            next_taken,
            retired.as_str(),
            first.as_str(),
            second.as_str()
        ),
        (FAILED, true, "retired true", STAGED_ONCE, NEVER_STAGED),
        "retired after the loop's next take"
    );

    // The retirement asked at once after the failed turn's answer.
    let mut staged = 0;
    for _ in 0..12 {
        let (failed, _next_taken, retired, first, second) = behind_a_failed_start(false).await;
        assert_eq!(
            (failed.as_str(), retired.as_str(), second.as_str()),
            (FAILED, "retired true", NEVER_STAGED),
            "retired at once: the failed turn, the retirement, the second input behind it"
        );
        assert!(
            staged_or_not(&first),
            "retired at once: the first input behind the failed turn: {first}"
        );
        if first == STAGED_ONCE {
            staged += 1;
        }
    }
    println!("retired at once: the first input behind was staged in {staged} of 12 runs");
}

/// The cause of the failed start persists over two starts: each start's
/// turn fails, an input is queued behind each, and the runtime is retired
/// each time. The input behind each failed turn ends at that retirement
/// (staged or not, as above), and the second start sees nothing of the
/// first: it starts no run before its own new input, never takes the input
/// queued behind the first start, and that input's stored record is not
/// touched again.
#[tokio::test]
async fn a_failed_start_whose_cause_persists_over_two_starts() {
    let machine = persistent_machine();
    let session_id = SessionId::new();
    let mut earlier: Option<(InputId, String)> = None;
    for start in 1..=2 {
        let registration = Registration::of(&machine, &session_id, usize::MAX).await;
        let run_before_any_new_input = registration
            .apply_started(1, Duration::from_millis(500))
            .await;
        let (_failing, failing_waiter) =
            accept(&machine, &session_id, event(&format!("fails-{start}"))).await;
        assert!(registration.apply_started(1, BOUND).await);
        let (behind, behind_waiter) =
            accept(&machine, &session_id, prompt(&format!("behind-{start}"))).await;
        registration.gate.add_permits(1);
        let failed = answer(failing_waiter).await;
        let retired = retire(&machine, &session_id, &registration).await;
        assert_eq!(
            (run_before_any_new_input, failed.as_str(), retired.as_str()),
            (false, FAILED, "retired true"),
            "start {start}: (a run before any new input, the failed turn, the retirement)"
        );
        let behind_end = ended(&machine, &session_id, &registration, &behind, behind_waiter).await;
        assert!(
            staged_or_not(&behind_end),
            "start {start}: the input behind the failed turn: {behind_end}"
        );
        if let Some((earlier, record_after_its_start)) = &earlier {
            assert_eq!(
                (
                    registration.took(earlier),
                    stored(&machine, &session_id, earlier).await
                ),
                (false, record_after_its_start.clone()),
                "start {start}: (took the input queued behind start 1, that input's stored \
                 record)"
            );
        }
        let record = stored(&machine, &session_id, &behind).await;
        earlier.get_or_insert((behind, record));
    }
}

/// An inbound peer request is queued behind a turn that fails at its start,
/// the runtime is retired, and the session is registered again. The request
/// is recorded and handed over as the comms drain does it: the machine's
/// inbound request record first, then the input. The request ends at the
/// retirement like every other input behind the failed turn. The row also
/// pins, as measured, what the machine holds for the request at the next
/// registration, with and without an input queued for it; what it should
/// hold there is tracked as a follow-up.
#[tokio::test]
async fn an_inbound_peer_request_behind_a_failed_start() {
    use meerkat_core::handles::PeerInteractionHandle as _;

    let machine = persistent_machine();
    let session_id = SessionId::new();
    let registration = Registration::of(&machine, &session_id, 1).await;
    let (_failing, failing_waiter) = accept(&machine, &session_id, event("fails")).await;
    assert!(registration.apply_started(1, BOUND).await);

    let corr = meerkat_core::PeerCorrelationId::new();
    let requester = meerkat_core::comms::PeerId::new();
    machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("the session's bindings")
        .peer_interaction()
        .request_received(corr, meerkat_core::types::HandlingMode::Queue, requester)
        .expect("the inbound request is recorded");
    let (request, request_waiter) = accept(
        &machine,
        &session_id,
        peer_request("request", corr, requester),
    )
    .await;
    assert_eq!(
        inbound_request_on_record(&machine, &session_id, corr).await,
        "true (inbound requests held: 1)",
        "the inbound request is on record while its input is queued"
    );
    registration.gate.add_permits(1);
    let failed = answer(failing_waiter).await;
    let retired = retire(&machine, &session_id, &registration).await;
    assert_eq!(
        (failed.as_str(), retired.as_str()),
        (FAILED, "retired true"),
        "(the failed turn, the retirement)"
    );
    let request_end = ended(
        &machine,
        &session_id,
        &registration,
        &request,
        request_waiter,
    )
    .await;
    assert!(
        staged_or_not(&request_end),
        "the request behind the failed turn: {request_end}"
    );
    let record_after_the_retirement = stored(&machine, &session_id, &request).await;

    let next = Registration::of(&machine, &session_id, 0).await;
    assert_eq!(
        (
            next.apply_started(1, Duration::from_millis(500)).await,
            next.took(&request),
            inbound_request_on_record(&machine, &session_id, corr).await,
            stored(&machine, &session_id, &request).await,
        ),
        (
            false,
            false,
            "false (inbound requests held: 0)".to_string(),
            record_after_the_retirement,
        ),
        "at the next registration: (a run started, it took the request, the inbound request \
         on record, the request's stored record)"
    );
    next.gate.close();

    // A recorded inbound request with no input queued for it, over a
    // retirement and a new registration.
    let session_id = SessionId::new();
    let registration = Registration::of(&machine, &session_id, 0).await;
    let corr = meerkat_core::PeerCorrelationId::new();
    machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("the session's bindings")
        .peer_interaction()
        .request_received(corr, meerkat_core::types::HandlingMode::Queue, requester)
        .expect("the inbound request is recorded");
    let while_registered = inbound_request_on_record(&machine, &session_id, corr).await;
    let retired = retire(&machine, &session_id, &registration).await;
    let next = Registration::of(&machine, &session_id, 0).await;
    let at_the_next_registration = inbound_request_on_record(&machine, &session_id, corr).await;
    assert_eq!(
        (
            while_registered.as_str(),
            retired.as_str(),
            at_the_next_registration.as_str()
        ),
        (
            "true (inbound requests held: 1)",
            "retired true",
            "false (inbound requests held: 0)"
        ),
        "a recorded inbound request with no queued input: (on record while registered, the \
         retirement, on record at the next registration)"
    );
    next.gate.close();
}
