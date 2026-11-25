//! The poll of runs that wait for their resumed-run admission
//! ([`MeerkatMachine::pending_resumed_run`],
//! [`MeerkatMachine::pending_resumed_runs`]) does not wait for a session
//! whose state stays held: it answers that the session is not readable now,
//! once the read bound has passed, and still answers every other session.
//!
//! The row holds a session's driver itself, with nothing degraded: no store
//! commit is in flight and no registration needs a reload. The rows that
//! poll through the public surface retry a not-readable answer; this row is
//! the one that asserts it.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::*;
use std::time::Duration;

use meerkat_core::lifecycle::RunId;

use crate::resumed_run_admission::{
    PENDING_RESUMED_RUN_READ_BOUND, PendingResumedRunRead, RESUMED_RUN_ADMISSION_REASON_MAX_BYTES,
    ResumedRunAdmissionState, ResumedRunAdmissionUndecidedCause,
};
use crate::store::RuntimeStore;

/// How long an answer may take on a loaded host; the read itself waits the
/// read bound for each session it cannot read.
const ANSWER_BOUND: Duration = Duration::from_secs(5);

async fn driver_of(machine: &MeerkatMachine, session_id: &SessionId) -> SharedDriver {
    let sessions = machine.sessions.read().await;
    Arc::clone(
        &sessions
            .get(session_id)
            .expect("registered session entry")
            .driver,
    )
}

/// Register a session and mark a run of it resumable, as a registration
/// that recovers a run a restart left open does.
async fn a_session_marking_a_run(machine: &MeerkatMachine, session_id: &SessionId) -> RunId {
    machine
        .register_session(session_id.clone())
        .await
        .expect("the session registers");
    let run_id = RunId::new();
    let driver = driver_of(machine, session_id).await;
    assert!(
        driver
            .lock()
            .await
            .shell_driver_mut()
            .mark_run_resumable(&run_id),
        "the machine marks the run"
    );
    run_id
}

fn not_readable(read: Option<&PendingResumedRunRead>, session_id: &SessionId) -> bool {
    matches!(
        read,
        Some(PendingResumedRunRead::NotReadableNow { session_id: named }) if named == session_id
    )
}

/// Three sessions each mark a run; the last one's run is held for its
/// admission with a reason past the bound. While the row holds the middle
/// session's driver:
///
/// - the poll of that session answers `NotReadableNow`, naming the session,
///   after the read bound and without waiting for the driver;
/// - the poll of every session answers all three in session id order: the
///   first session's run, the middle session not readable now, the last
///   session's held run with its round and its reason cut at the bound;
/// - the polls change nothing: once the driver is free, the middle session
///   answers its marked run, not held, and the last session's round count
///   is the one round it noted.
#[tokio::test]
async fn a_session_whose_state_stays_held_is_polled_as_not_readable_now_and_the_others_are_answered()
 {
    let machine = Arc::new(
        MeerkatMachine::persistent(
            Arc::new(crate::store::InMemoryRuntimeStore::new()) as Arc<dyn RuntimeStore>,
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("persistent machine"),
    );
    let mut sessions = [SessionId::new(), SessionId::new(), SessionId::new()];
    sessions.sort_by_key(ToString::to_string);
    let [first, middle, last] = sessions;
    let first_run = a_session_marking_a_run(&machine, &first).await;
    let middle_run = a_session_marking_a_run(&machine, &middle).await;
    let last_run = a_session_marking_a_run(&machine, &last).await;
    let wide_reason = "w".repeat(RESUMED_RUN_ADMISSION_REASON_MAX_BYTES * 2);
    driver_of(&machine, &last)
        .await
        .lock()
        .await
        .hold_resumed_run_for_admission(
            &last_run,
            ResumedRunAdmissionUndecidedCause::AnsweredUnavailable,
            &wide_reason,
        )
        .expect("the machine holds the last session's marked run");

    let middle_driver = driver_of(&machine, &middle).await;
    let held = middle_driver.lock().await;

    let asked = std::time::Instant::now();
    let one = tokio::time::timeout(ANSWER_BOUND, machine.pending_resumed_run(&middle))
        .await
        .expect("the poll of the held session answers");
    let waited = asked.elapsed();
    assert!(
        not_readable(one.as_ref(), &middle),
        "the held session answers not readable now, naming itself: {one:?}"
    );
    assert!(
        waited >= PENDING_RESUMED_RUN_READ_BOUND,
        "the poll gave the session its read bound first: {waited:?}"
    );

    let all = tokio::time::timeout(ANSWER_BOUND, machine.pending_resumed_runs())
        .await
        .expect("the poll of every session answers");
    let [first_read, middle_read, last_read] = all.as_slice() else {
        panic!("one answer for each of the three sessions: {all:?}");
    };
    let PendingResumedRunRead::Pending(first_pending) = first_read else {
        panic!("the first session is readable: {first_read:?}");
    };
    assert_eq!(
        (&first_pending.session_id, &first_pending.run_id),
        (&first, &first_run)
    );
    assert_eq!(
        first_pending.admission,
        ResumedRunAdmissionState::MarkedNotHeld
    );
    assert!(
        first_pending.marked_for.is_some(),
        "the registration stamped the mark it applied"
    );
    assert!(
        not_readable(Some(middle_read), &middle),
        "the held session keeps its place, not readable now: {middle_read:?}"
    );
    let PendingResumedRunRead::Pending(last_pending) = last_read else {
        panic!("the last session is readable: {last_read:?}");
    };
    assert_eq!(
        (&last_pending.session_id, &last_pending.run_id),
        (&last, &last_run)
    );
    let ResumedRunAdmissionState::Held(last_held) = &last_pending.admission else {
        panic!("the last session's run reads held: {last_pending:?}");
    };
    let undecided = last_held
        .undecided
        .as_ref()
        .expect("the held run's noted round");
    assert_eq!(
        (undecided.rounds, undecided.cause, undecided.reason.len()),
        (
            1,
            ResumedRunAdmissionUndecidedCause::AnsweredUnavailable,
            RESUMED_RUN_ADMISSION_REASON_MAX_BYTES
        ),
        "the answer carries the one round, its cause and the reason cut at the bound"
    );
    assert!(wide_reason.starts_with(&undecided.reason));

    drop(held);
    let PendingResumedRunRead::Pending(middle_pending) = machine
        .pending_resumed_run(&middle)
        .await
        .expect("the freed session answers its marked run")
    else {
        panic!("the freed session is readable");
    };
    assert_eq!(middle_pending.run_id, middle_run);
    assert_eq!(
        middle_pending.admission,
        ResumedRunAdmissionState::MarkedNotHeld,
        "the polls changed nothing of the session they could not read"
    );
    let PendingResumedRunRead::Pending(last_again) = machine
        .pending_resumed_run(&last)
        .await
        .expect("the last session still answers its held run")
    else {
        panic!("the last session is readable");
    };
    let ResumedRunAdmissionState::Held(last_held) = &last_again.admission else {
        panic!("the last session's run still reads held: {last_again:?}");
    };
    assert_eq!(
        last_held
            .undecided
            .as_ref()
            .map(|undecided| undecided.rounds),
        Some(1),
        "the polls moved no round count"
    );
}
