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
use super::publication_source_wait::{
    Found, approved_script, expiring_script, finds_with, finds_with_hold_at, reply,
};

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

// A helper submission's binder, inside Core's governed bind, meets the held
// collection at its first source check. It refuses with nothing staged, and
// the submission waits for the collection outside the bind, holding nothing,
// and dispatches the same input again; one custody attempt is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submission_bind_waits_out_a_held_source_collection() {
    let found = finds_with_hold_at(Site::SubmitBind, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
    assert_eq!(found.attempt_writes, 1, "one custody attempt was written");
}

// The binder meets the held collection at its custody attempt's source
// checks, the attempt's write just done: the submission waits and binds
// again, and the equal pending attempt is reused, never written again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submission_persist_waits_out_a_held_source_collection_after_its_attempt_write() {
    let found = finds_with_hold_at(Site::SubmitPersisted, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
    assert_eq!(
        found.attempt_writes, 1,
        "the custody attempt was written once and reused"
    );
}

// Held past the bound at the binder's first source check: the submission
// answers at its one deadline, before the hold ended, and the Find answers
// retry_safe with nothing admitted; no custody attempt was written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submission_bind_held_past_the_bound_answers_retry_safe() {
    let hold = held_past_the_bound();
    let found = finds_with_hold_at(Site::SubmitBind, 1, hold, 0, false).await;
    assert!(found.turn_completed, "the original's turn completed");
    assert_eq!(found.feedback.len(), 1, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert_eq!(
        reply(first)["worker"]["state"],
        "retry_safe",
        "{}",
        reply(first)
    );
    assert!(
        found.answered_after >= OWNER_CHANGE_WAIT_BOUND - Duration::from_millis(500)
            && found.answered_after < hold.duration,
        "answered once the submission's deadline ran out, before the hold ended: {:?}",
        found.answered_after
    );
    assert_eq!(found.attempt_writes, 0, "no custody attempt was written");
}

// The binder meets the held collection at its custody attempt's source
// checks, before the attempt's write: the submission waits and binds again,
// and the attempt is written once, by the bind that went.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submission_persist_waits_out_a_held_source_collection_before_its_attempt_write() {
    let found = finds_with_hold_at(Site::SubmitPersist, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
    assert_eq!(
        found.attempt_writes, 1,
        "the custody attempt was written once"
    );
}

// The binder meets the held collection at its final source check, the
// custody attempt done: the submission waits and binds again, the equal
// pending attempt is reused, and the input is admitted once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submission_final_check_waits_out_a_held_source_collection() {
    let found = finds_with_hold_at(Site::SubmitFinalCheck, 1, collection_held(), 0, false).await;
    assert_admitted_after(&found, HOLD);
    assert_eq!(
        found.attempt_writes, 1,
        "the custody attempt was written once and reused"
    );
}

// The binder meets the held collection at its first source check, and the
// submission is delayed past its one deadline once its dispatch returned, the
// hold long over (a task descheduled there): no late dispatch is made, and
// the Find answers retry_safe with nothing admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submission_past_its_deadline_after_a_held_bind_dispatches_no_more() {
    let delayed = Hold {
        owner_change: false,
        duration: OWNER_CHANGE_WAIT_BOUND + Duration::from_millis(300),
    };
    let found = finds_with(
        approved_script(),
        Site::SubmitBind,
        1,
        collection_held(),
        0,
        false,
        &[(Site::SubmitReturned, 1, delayed)],
    )
    .await;
    assert!(found.turn_completed, "the original's turn completed");
    assert_eq!(found.feedback.len(), 1, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert_eq!(
        reply(first)["worker"]["state"],
        "retry_safe",
        "{}",
        reply(first)
    );
    assert_eq!(found.site_hits, 1, "the binder ran once: no late dispatch");
    assert_eq!(found.attempt_writes, 0, "no custody attempt was written");
}

// The binder meets the held collection at its first source check, and the
// submission is delayed past the helper's source deadline once its dispatch
// returned: the expired source is denied, never attempted again, and nothing
// is admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submission_past_its_source_deadline_after_a_held_bind_is_denied() {
    const SETUP_TIMEOUT_MS: u64 = 5_000;
    let delayed = Hold {
        owner_change: false,
        duration: Duration::from_millis(SETUP_TIMEOUT_MS + 200),
    };
    let found = finds_with(
        expiring_script(SETUP_TIMEOUT_MS),
        Site::SubmitBind,
        1,
        collection_held(),
        0,
        false,
        &[(Site::SubmitReturned, 1, delayed)],
    )
    .await;
    assert!(found.turn_completed, "the original's turn completed");
    assert_eq!(found.feedback.len(), 1, "{:?}", found.feedback);
    let first = &found.feedback[0];
    assert!(first.is_error, "{}", reply(first));
    let expected = meerkat_core::ToolError::AccessDenied {
        name: crate::FIND_AND_INSTALL.into(),
    }
    .to_error_payload();
    assert_eq!(reply(first), expected, "an expired source is a denial");
    assert_eq!(found.site_hits, 1, "the binder ran once: no late dispatch");
    assert_eq!(found.attempt_writes, 0, "no custody attempt was written");
}
