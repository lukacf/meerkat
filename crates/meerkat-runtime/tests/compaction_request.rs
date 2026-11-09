//! A host's compaction request (`MeerkatMachine::request_compaction`): the
//! machine records it on the session's registration, the current run's
//! compaction check consumes it once, and a registration that does not serve
//! refuses it without recording anything. Nothing here wakes or registers a
//! session.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::Arc;

use meerkat_core::handles::TurnStateHandle;
use meerkat_core::lifecycle::RunId;
use meerkat_core::turn_execution_authority::{ContentShape, TurnPrimitiveKind};
use meerkat_core::types::SessionId;
use meerkat_runtime::{
    CompactionRequestReceipt, LogicalRuntimeId, MeerkatMachine, RuntimeDriverError, RuntimeState,
    SessionServiceRuntimeExt as _,
};

/// A session registered on an ephemeral machine with its runtime binding
/// prepared, and the session's turn-state handle (the one its agent gets).
async fn registered_session() -> (Arc<MeerkatMachine>, SessionId, Arc<dyn TurnStateHandle>) {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    let bindings = machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("prepare the session's runtime bindings");
    let turn = Arc::clone(bindings.turn_state());
    (machine, session_id, turn)
}

/// Start a conversation run whose turn then calls the model: the point at
/// which the machine emits the run's compaction check.
fn start_conversation_run(turn: &dyn TurnStateHandle, run_id: &RunId) {
    turn.start_conversation_run(
        run_id.clone(),
        TurnPrimitiveKind::ConversationTurn,
        ContentShape::Conversation,
        false,
        false,
        0,
    )
    .expect("start a conversation run");
    turn.primitive_applied(run_id.clone())
        .expect("the run's turn calls the model");
}

/// An idle registration records a request; a second request before any check
/// consumed it changes nothing.
#[tokio::test]
async fn an_idle_registration_records_one_request() {
    let (machine, session_id, _turn) = registered_session().await;
    assert_eq!(
        machine.request_compaction(&session_id).await.unwrap(),
        CompactionRequestReceipt::Requested
    );
    assert_eq!(
        machine.request_compaction(&session_id).await.unwrap(),
        CompactionRequestReceipt::AlreadyRequested,
        "a held request is not recorded twice"
    );
}

/// The next run's first compaction check consumes a request recorded while
/// the session was idle, and only once.
#[tokio::test]
async fn the_next_runs_first_check_consumes_an_idle_request_once() {
    let (machine, session_id, turn) = registered_session().await;
    assert_eq!(
        machine.request_compaction(&session_id).await.unwrap(),
        CompactionRequestReceipt::Requested
    );
    let run_id = RunId::new();
    start_conversation_run(turn.as_ref(), &run_id);
    assert!(
        turn.consume_compaction_request(run_id.clone()).unwrap(),
        "the run's first check consumes the request"
    );
    assert!(
        !turn.consume_compaction_request(run_id).unwrap(),
        "a later check of the run finds none"
    );
    assert_eq!(
        machine.request_compaction(&session_id).await.unwrap(),
        CompactionRequestReceipt::Requested,
        "a consumed request no longer holds the next one"
    );
}

/// A request that arrives while a turn runs leaves the turn running, and the
/// turn's next compaction check consumes it. A consumption asked while the
/// turn waits for its tools, or by another run, takes nothing.
#[tokio::test]
async fn a_running_turns_next_check_consumes_a_request_made_during_it() {
    let (machine, session_id, turn) = registered_session().await;
    let run_id = RunId::new();
    start_conversation_run(turn.as_ref(), &run_id);
    assert!(
        !turn.consume_compaction_request(run_id.clone()).unwrap(),
        "the run's first check finds no request"
    );
    turn.llm_returned_tool_calls(run_id.clone(), 1)
        .expect("the model asks for one tool");
    assert_eq!(
        machine.request_compaction(&session_id).await.unwrap(),
        CompactionRequestReceipt::Requested,
        "a running registration records the request"
    );
    assert_eq!(
        machine.runtime_state(&session_id).await.unwrap(),
        RuntimeState::Running,
        "the turn keeps running"
    );
    assert!(
        !turn.consume_compaction_request(run_id.clone()).unwrap(),
        "no check runs while the turn waits for its tools"
    );
    assert!(
        !turn.consume_compaction_request(RunId::new()).unwrap(),
        "another run's check consumes nothing"
    );
    turn.tool_calls_resolved(run_id.clone())
        .expect("the tool results are in and the turn calls the model again");
    assert!(
        turn.consume_compaction_request(run_id.clone()).unwrap(),
        "the turn's next check consumes the request"
    );
    assert!(!turn.consume_compaction_request(run_id).unwrap());
}

/// A re-registration of the same session keeps a held request, as it keeps
/// the session's run-start holds: preparing the session's bindings again
/// stages the same registration.
#[tokio::test]
async fn a_same_session_re_registration_keeps_the_request() {
    let (machine, session_id, _turn) = registered_session().await;
    assert_eq!(
        machine.request_compaction(&session_id).await.unwrap(),
        CompactionRequestReceipt::Requested
    );
    let _again = machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("prepare the same session's bindings again");
    assert_eq!(
        machine.request_compaction(&session_id).await.unwrap(),
        CompactionRequestReceipt::AlreadyRequested,
        "the request survives the same session's re-registration"
    );
}

/// A retired registration refuses a request with its state, and records
/// nothing.
#[tokio::test]
async fn a_retired_registration_refuses_the_request() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register the session");
    meerkat_runtime::traits::RuntimeControlPlane::retire(
        &*machine,
        &LogicalRuntimeId::for_session(&session_id),
    )
    .await
    .expect("retire the session's runtime");
    let refusal = machine
        .request_compaction(&session_id)
        .await
        .expect_err("a retired runtime refuses the request");
    assert!(
        matches!(
            refusal,
            RuntimeDriverError::NotReady {
                state: RuntimeState::Retired
            }
        ),
        "{refusal:?}"
    );
}

/// A session with no registration in this process is refused, and the
/// request registers nothing: a dormant session is never woken for it.
#[tokio::test]
async fn an_unregistered_session_is_refused_and_stays_unregistered() {
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    let refusal = machine
        .request_compaction(&session_id)
        .await
        .expect_err("no registration holds the request");
    assert!(
        matches!(
            refusal,
            RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed
            }
        ),
        "{refusal:?}"
    );
    assert!(
        !machine.contains_session(&session_id).await,
        "the request registers nothing"
    );
}
