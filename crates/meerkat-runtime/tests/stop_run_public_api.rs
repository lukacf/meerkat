#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Public-API contract for the run-fenced Stop as an embedding host reaches
//! it: an `Option<Arc<MeerkatMachine>>` runtime handle (the shape a MobKit
//! gateway keeps as its session runtime) calling `stop_run` directly, with the
//! receipt types taken only from the crate root. A stale run id is the
//! `NotCurrent` receipt, which such a host treats as a no-op; it must never be
//! an error that tempts a fallback to ambient cancellation.

use std::sync::Arc;

use meerkat_core::lifecycle::RunId;
use meerkat_core::types::SessionId;
use meerkat_runtime::{MeerkatMachine, RunStopContributor, RunStopReceipt, RuntimeState};

/// Exhaustively names every public receipt shape through public paths only.
fn describe(receipt: &RunStopReceipt) -> &'static str {
    match receipt {
        RunStopReceipt::Stopped { contributors, .. } => {
            let _: &Vec<RunStopContributor> = contributors;
            "stopped"
        }
        RunStopReceipt::NotCurrent { .. } => "not_current",
        RunStopReceipt::NotStoppable { state, .. } => {
            let _: &RuntimeState = state;
            "not_stoppable"
        }
        _ => "future",
    }
}

#[tokio::test]
async fn embedding_host_stop_of_a_stale_run_is_a_not_current_no_op() {
    let session_runtime: Option<Arc<MeerkatMachine>> = Some(Arc::new(MeerkatMachine::ephemeral()));
    let machine = session_runtime.as_ref().expect("runtime handle");
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");

    let stale = RunId::new();
    let receipt = machine
        .stop_run(&session_id, &stale, "host stopped a stale selection")
        .await
        .expect("a stale stop is a typed receipt, never an error");
    assert_eq!(describe(&receipt), "not_current");
    match receipt {
        RunStopReceipt::NotCurrent {
            run_id,
            current_run_id,
        } => {
            assert_eq!(run_id, stale);
            assert_eq!(current_run_id, None);
        }
        other => panic!("expected NotCurrent, got {other:?}"),
    }

    // An unregistered session has no current run either: still NotCurrent.
    let receipt = machine
        .stop_run(&SessionId::new(), &stale, "unknown session")
        .await
        .expect("unknown session stop");
    assert_eq!(describe(&receipt), "not_current");
}
