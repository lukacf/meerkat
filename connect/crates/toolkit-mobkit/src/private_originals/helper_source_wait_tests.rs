//! A Find's helper source steps meeting another owner's short section on the
//! owner's originals collection, or an owner change of the shared
//! publication, at the source hold probe's other sites:
//! - `Retain`: the work step's first binding write just done, before its
//!   contributors' check and its publication;
//! - `Recover`: the Find's recovery of that bound source (`advance.recover_helper`),
//!   an exact replay that writes nothing, before the same check and
//!   publication;
//! - `Resolve`: a retention resolving the helper's issued lineage, the work
//!   step's and the recovery's own;
//! - `Observe`: the Find's live-replay helper observation
//!   (`advance.observe_helper`);
//! - `FencedAfter`: a fenced helper create or read just done, before its
//!   source check under the writer;
//! - `IntentsWriter`, `Intents` and `IntentsWritten`: a helper's grant
//!   issuance taking the source writer, then before and after its intent
//!   write;
//! - `FirstWrite`: a first binding write just before it begins its owner
//!   change, the hold lasting into the change, where the write tries the
//!   collection;
//! - `PrepareBind`: a helper preparation's capture binder;
//! - `Submit`: a helper submission's source check.
//!
//! A held collection is waited out within Core's owner-change bound, and an
//! owner change met by the lineage resolution is a stale read, waited out
//! with the step's one churn; only the check, the read or the publication is
//! made again, never the binding write. Past the bounds the refusal is typed
//! `tool_unavailable`, never a denial. Another actor's owner change begun
//! while a bind waits is not held up by it.
use super::publication_source_wait::{Found, finds_with_hold_at, reply};

use crate::source_hold_probe::{Hold, Site};
use meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND;
use std::time::Duration;

const HOLD: Duration = Duration::from_millis(150);

/// The Find admitted its helper once the hold was waited out: one source row,
/// its controller grant issued once and its source retained once.
fn assert_admitted_after(found: &Found, hold: Duration) {
    assert!(found.turn_completed, "the original's turn completed");
    assert_eq!(found.feedback.len(), 1, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert!(
        !first.is_error,
        "the hold is waited out, not refused: {}",
        reply(first)
    );
    assert_eq!(
        reply(first)["worker"]["state"],
        "input_admitted",
        "{}",
        reply(first)
    );
    assert_eq!(found.helpers, 1, "the helper was retained once");
    assert_eq!(
        found.issued.len(),
        1,
        "the controller grant was issued once"
    );
    assert_eq!(found.rows.len(), 1, "one canonical source row");
    assert!(
        found.answered_after >= hold - Duration::from_millis(50),
        "the step waited for the hold: {:?}",
        found.answered_after
    );
}

fn collection_held() -> Hold {
    Hold {
        owner_change: false,
        duration: HOLD,
    }
}

fn change_held() -> Hold {
    Hold {
        owner_change: true,
        duration: HOLD,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retain_after_its_binding_write_waits_out_a_held_source_collection() {
    let found = finds_with_hold_at(Site::Retain, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recover_replay_waits_out_a_held_source_collection() {
    let found = finds_with_hold_at(Site::Recover, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retain_lineage_resolution_waits_out_an_owner_change() {
    let found = finds_with_hold_at(Site::Resolve, 1, change_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recover_lineage_resolution_waits_out_an_owner_change() {
    let found = finds_with_hold_at(Site::Resolve, 2, change_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_observation_waits_out_a_held_source_collection() {
    let found = finds_with_hold_at(Site::Observe, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

// Another actor's owner change on the owner's publication, begun 30 ms into a
// bind's wait for the held collection: it needs only the publication writer,
// never the source writer the bind keeps, so it begins at once, and the bind
// still waits the hold out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owner_change_begun_while_a_bind_waits_is_not_held_up() {
    let found = finds_with_hold_at(Site::Retain, 1, collection_held(), 0, true).await;
    assert_admitted_after(&found, HOLD);
    let waited = found
        .concurrent_change
        .expect("the other actor's owner change began");
    assert!(
        waited < Duration::from_millis(50),
        "the other actor's owner change began at once: {waited:?}"
    );
}

// The negative control: the collection held after the binding write past
// Core's owner-change bound, with no owner change. The bind's one budget
// (from its start) runs out before the hold ends; the Find answers
// tool_unavailable then, and nothing is retained or admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retain_held_past_the_bound_is_refused_typed() {
    let hold = OWNER_CHANGE_WAIT_BOUND + Duration::from_millis(500);
    let found = finds_with_hold_at(
        Site::Retain,
        1,
        Hold {
            owner_change: false,
            duration: hold,
        },
        0,
        false,
    )
    .await;
    assert!(found.turn_completed, "the original's turn completed");
    assert_eq!(found.feedback.len(), 1, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert!(first.is_error, "{}", reply(first));
    assert_eq!(
        reply(first)["error"],
        "tool_unavailable",
        "{}",
        reply(first)
    );
    assert_eq!(found.helpers, 0, "nothing was retained");
    assert!(
        found.answered_after >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(100)
            && found.answered_after < hold,
        "refused once the bind's bound ran out, before the hold ended: {:?}",
        found.answered_after
    );
}

// An owner change held at the lineage resolution past the stale-read churn's
// bound: the retention stays typed, tool_unavailable, and never answers
// access_denied for a change it could not see the end of. The hold ends one
// second after that bound, within the original's next model request's own
// wait for the change, so the turn completes; a refusal at once (the old
// denial) leaves the change to outlast that wait, so the turn's completion
// is not part of this row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retain_lineage_resolution_past_the_bounds_is_unavailable_never_denied() {
    let hold = OWNER_CHANGE_WAIT_BOUND + Duration::from_secs(1);
    let found = finds_with_hold_at(
        Site::Resolve,
        1,
        Hold {
            owner_change: true,
            duration: hold,
        },
        0,
        false,
    )
    .await;
    assert_eq!(found.feedback.len(), 1, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert!(first.is_error, "{}", reply(first));
    assert_eq!(
        reply(first)["error"],
        "tool_unavailable",
        "a change is never a denial: {}",
        reply(first)
    );
    assert_eq!(found.helpers, 0, "nothing was retained");
}

/// The Find refused typed once its step's one deadline ran out, before the
/// hold ended: tool_unavailable, nothing retained.
fn assert_refused_at_the_bound(found: &Found, hold: Duration) {
    assert!(found.turn_completed, "the original's turn completed");
    assert_eq!(found.feedback.len(), 1, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert!(first.is_error, "{}", reply(first));
    assert_eq!(
        reply(first)["error"],
        "tool_unavailable",
        "{}",
        reply(first)
    );
    assert_eq!(found.helpers, 0, "nothing was retained");
    assert!(
        found.answered_after >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(500)
            && found.answered_after < hold,
        "refused once the step's deadline ran out, before the hold ended: {:?}",
        found.answered_after
    );
}

fn held_past_the_bound() -> Hold {
    Hold {
        owner_change: false,
        duration: OWNER_CHANGE_WAIT_BOUND + Duration::from_millis(500),
    }
}

// The work step's fenced create of the helper's work item just done: its
// check of the helper's sources under the source writer meets the held
// collection and waits it out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_helper_after_its_fenced_create_waits_out_a_held_source_collection() {
    let found = finds_with_hold_at(Site::FencedAfter, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

// The Find's live-replay observation, its fenced read just done: the same
// source check waits the hold out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn observed_helper_after_its_fenced_read_waits_out_a_held_source_collection() {
    let found = finds_with_hold_at(Site::FencedAfter, 2, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

// The helper's grant issuance meets the source writer held by another
// writer before its intent write: it waits for the writer within the
// issuance's deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issuance_waits_out_a_held_source_writer_before_its_intent_write() {
    let found = finds_with_hold_at(Site::IntentsWriter, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

// With the writer taken, the issuance's source check before its intent
// write meets the held collection and waits it out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issuance_waits_out_a_held_source_collection_before_its_intent_write() {
    let found = finds_with_hold_at(Site::Intents, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

// After the intent write, the issuance's source check meets the held
// collection and waits it out; the intent is never written again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issuance_waits_out_a_held_source_collection_after_its_intent_write() {
    let found = finds_with_hold_at(Site::IntentsWritten, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

// The first binding write tries the collection inside its owner change and
// finds it held: the change ends with nothing changed, the collection is
// waited for outside it, and the change begins again, before the CAS.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_binding_write_waits_out_a_held_source_collection_outside_its_change() {
    let found = finds_with_hold_at(Site::FirstWrite, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

// The work step's helper preparation: its capture binder meets the held
// collection inside Core's governed bind. It refuses with nothing captured;
// the preparation waits for the collection outside the bind and binds again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_preparation_binds_again_after_a_held_source_collection() {
    let found = finds_with_hold_at(Site::PrepareBind, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

// The same at the Find's own helper preparation after the work step
// (`advance.prepare_helper`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn find_helper_preparation_binds_again_after_a_held_source_collection() {
    let found = finds_with_hold_at(Site::PrepareBind, 2, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

// The helper's submission (`advance.submit_helper`): its source check meets
// the held collection and waits it out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submission_waits_out_a_held_source_collection() {
    let found = finds_with_hold_at(Site::Submit, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
}

// Negative controls, held past Core's owner-change bound: each step answers
// tool_unavailable once its one deadline ran out, before the hold ended, and
// nothing is retained.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issuance_writer_held_past_the_bound_is_refused_typed() {
    let hold = held_past_the_bound();
    let found = finds_with_hold_at(Site::IntentsWriter, 1, hold, 0, false).await;
    assert_refused_at_the_bound(&found, hold.duration);
    assert!(found.issued.is_empty(), "nothing was issued");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_binding_write_held_past_the_bound_is_refused_typed() {
    let hold = held_past_the_bound();
    let found = finds_with_hold_at(Site::FirstWrite, 1, hold, 0, false).await;
    assert_refused_at_the_bound(&found, hold.duration);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_preparation_held_past_the_bound_is_refused_typed() {
    let hold = held_past_the_bound();
    let found = finds_with_hold_at(Site::PrepareBind, 1, hold, 0, false).await;
    assert_refused_at_the_bound(&found, hold.duration);
    assert!(found.issued.is_empty(), "nothing was issued");
}

// A control of the shipped first-attempt window, not a fix: the helper
// binder writes the helper's first custody attempt and is refused before its
// enrollment. Another writer holds the source writer from the submission's
// source check (`Submit`) past the helper's source deadline, so the attempt's
// write waits, lands, and the binder's deadline check after it refuses.
// Nothing is admitted: the Find answers native_outcome_unavailable, the
// helper's native session holds no input, no custody is enrolled and no
// settlement is retained. The written attempt stays pending with no
// observation spent. Nothing in this process visits it again (the cold census
// skips a live first binding); a send of the same input runs every check
// again before it could adopt it, and after a restart the cold census
// withdraws it without submitting anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn helper_first_attempt_refused_after_its_write_admits_nothing() {
    use super::{Fixture, Report, acquisition_denial};
    use crate::original_ingress::custody_attempt::{CustodyAttempt, CustodyDecision};
    use futures::FutureExt;
    use meerkat_runtime::SessionServiceRuntimeExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use toolkit_store::ToolkitStore;

    const SETUP_TIMEOUT_MS: u64 = 5_000;
    let script = acquisition_denial::Script::for_expiring_setup(SETUP_TIMEOUT_MS);
    let report = Report::new(script.tools().result_tool());
    let (fixture, _runtime_store) =
        Fixture::for_configured_helper_report(script.clone(), report.clone()).await;
    super::start_custody_recovery(&fixture);
    let owner = fixture.source.owner.clone();
    let submissions = Arc::new(AtomicUsize::new(0));
    let seen = submissions.clone();
    crate::source_hold_probe::install(Arc::new(move |at, _key| {
        if at == Site::Submit && seen.fetch_add(1, Ordering::SeqCst) == 0 {
            crate::source_hold_probe::retain(
                owner.hold_writer_for_test(Duration::from_millis(SETUP_TIMEOUT_MS + 1_000)),
            );
        }
        None
    }));
    let checked = std::panic::AssertUnwindSafe(tokio::time::timeout(
        Duration::from_secs(30),
        Box::pin(async {
            let first = fixture.submit().await;
            // The Find answers once its helper's bind was refused.
            while script.observations().0.is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let finished = fixture.finish(&first).await;
            assert!(
                finished.is_ok(),
                "the original's turn completed: {finished:?}"
            );
            assert_eq!(
                submissions.load(Ordering::SeqCst),
                1,
                "one helper submission"
            );
            let (feedback, _, _) = script.observations();
            assert_eq!(feedback.len(), 1, "{feedback:?}");
            let answer = reply(&feedback[0]);
            assert_eq!(
                answer["worker"]["state"], "native_outcome_unavailable",
                "the helper did not start: {answer}"
            );
            let helpers = fixture.source.helper_sources.lock().unwrap().clone();
            assert_eq!(helpers.len(), 1, "the helper's source was retained once");
            let held = &helpers[0];
            assert!(
                held.first_custody.lock().unwrap().is_none(),
                "no custody was enrolled"
            );
            assert!(
                held.handoffs_for_test().is_empty(),
                "no settlement was retained"
            );
            let row = fixture
                .store
                .get(&held.source_record().key)
                .await
                .unwrap()
                .unwrap();
            let attempt: CustodyAttempt =
                serde_json::from_value(row.data.body["custody_attempt"].clone())
                    .expect("the binder wrote the helper's first attempt");
            assert!(
                attempt.decision() == CustodyDecision::Pending,
                "the attempt stays pending"
            );
            assert!(
                !attempt.observation_entered(),
                "no observation was spent on it"
            );
            assert!(
                fixture
                    .machine
                    .input_state(attempt.session(), attempt.input_id())
                    .await
                    .unwrap()
                    .is_none(),
                "native never received the helper's input"
            );
        }),
    ))
    .catch_unwind()
    .await;
    fixture.source.stop_custody_recovery();
    crate::source_hold_probe::clear();
    report.release();
    report.leaf.context.lock().unwrap().take();
    let cleanup = fixture.shutdown().await;
    let recovery = fixture.source.join_custody_recovery().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(
        recovery.is_ok(),
        "the custody recovery joined: {recovery:?}"
    );
    match checked {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("the Find did not settle before the fixture deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
