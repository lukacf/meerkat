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
//!   (`advance.observe_helper`).
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
