//! Live-waiter gate parity for the sqlite schedule store (2026-07 P0).
//!
//! The claim scan treats an expired lease as "the deliverer is dead" — but a
//! delivery whose waiter is verifiably alive in the calling process is not
//! dead, only late on renewal. `ClaimDueRequest.live_waiter_occurrence_ids`
//! carries the driver's waiter registry snapshot; rows in that set must be
//! neither reclaimed (`LeaseExpired` + attempt+1: a duplicate turn) nor
//! misfired (the Skip-policy false-misfire arm). Post-crash the set is empty
//! and today's reclaim/misfire behavior stands. Rule 8: one semantic
//! condition, one terminal shape across backends — this mirrors the memory
//! store's gate tests in `meerkat-schedule`.

#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    ClaimDueRequest, CreateScheduleRequest, DeliveryReceiptStage, MisfirePolicy,
    MissingTargetPolicy, Occurrence, OccurrenceLifecycleInput, OccurrenceOrdinal, OccurrencePhase,
    OverlapPolicy, Schedule, ScheduleLifecycleInput, ScheduleStore, ScheduledSessionAction,
    SessionTargetBinding, TargetBinding, TriggerSpec,
};
use meerkat_store::SqliteScheduleStore;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

fn sample_schedule_request(name: &str) -> CreateScheduleRequest {
    CreateScheduleRequest {
        name: Some(name.to_string()),
        description: None,
        trigger: TriggerSpec::Once {
            due_at_utc: Utc::now() - Duration::seconds(1),
        },
        target: TargetBinding::session(SessionTargetBinding::ExactSession {
            session_id: SessionId::new(),
            action: ScheduledSessionAction::Prompt {
                prompt: ContentInput::from("scheduled prompt"),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        }),
        misfire_policy: MisfirePolicy::Skip,
        overlap_policy: OverlapPolicy::SkipIfRunning,
        missing_target_policy: MissingTargetPolicy::MarkMisfired,
        labels: BTreeMap::new(),
        planning_horizon_days: Some(1),
        planning_horizon_occurrences: Some(1),
    }
}

async fn commit_schedule(store: &SqliteScheduleStore, name: &str) -> Schedule {
    let mutator = Schedule::apply(
        None,
        ScheduleLifecycleInput::Create(sample_schedule_request(name)),
    )
    .expect("schedule creation should pass generated authority");
    let schedule = mutator.schedule.clone();
    store
        .commit_schedule_write(mutator.into_authorized_write())
        .await
        .expect("commit schedule");
    schedule
}

async fn commit_occurrence_due_at(
    store: &SqliteScheduleStore,
    schedule: &Schedule,
    due_at_utc: chrono::DateTime<Utc>,
) -> Occurrence {
    let write = Occurrence::planned_write_from_schedule(schedule, OccurrenceOrdinal(0), due_at_utc)
        .expect("occurrence planning should pass generated authority");
    let occurrence = write.occurrence().clone();
    store
        .commit_occurrence_write(write)
        .await
        .expect("commit occurrence");
    occurrence
}

fn claim_request(
    lease: Duration,
    live: BTreeSet<meerkat_schedule::OccurrenceId>,
) -> ClaimDueRequest {
    ClaimDueRequest {
        owner_id: "gate-test".to_string(),
        limit: 8,
        lease_duration: lease,
        live_waiter_occurrence_ids: live,
    }
}

/// Claim + dispatch + await through the occurrence authority, with no waiter
/// task and no renewal in this process — the durable footprint of a
/// deliverer from another (possibly crashed) process.
async fn claim_and_dispatch(store: &SqliteScheduleStore, lease: Duration) -> Occurrence {
    let claimed = store
        .claim_due_occurrences(claim_request(lease, BTreeSet::new()))
        .await
        .expect("claim due occurrences");
    let occurrence = claimed
        .claimed
        .into_iter()
        .next()
        .expect("a due occurrence should be claimed");
    let dispatch_mutator = occurrence
        .apply(OccurrenceLifecycleInput::DispatchStarted {
            correlation_id: Some("gate-test-dispatch".into()),
            at_utc: claimed.store_now_utc,
        })
        .expect("dispatch should pass generated authority");
    let dispatching = dispatch_mutator.occurrence.clone();
    store
        .commit_occurrence_write(dispatch_mutator.into_authorized_write())
        .await
        .expect("commit dispatch");
    let await_mutator = dispatching
        .apply(OccurrenceLifecycleInput::AwaitCompletion {
            at_utc: claimed.store_now_utc,
        })
        .expect("await should pass generated authority");
    let awaiting = await_mutator.occurrence.clone();
    store
        .commit_occurrence_write(await_mutator.into_authorized_write())
        .await
        .expect("commit await");
    awaiting
}

#[tokio::test]
async fn live_waiter_gate_blocks_expiry_reclaim_in_sqlite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");

    let schedule = commit_schedule(&store, "gate-expiry").await;
    let occurrence =
        commit_occurrence_due_at(&store, &schedule, Utc::now() - Duration::seconds(1)).await;
    let awaiting = claim_and_dispatch(&store, Duration::milliseconds(25)).await;
    assert_eq!(awaiting.occurrence_id, occurrence.occurrence_id);

    tokio::time::sleep(std::time::Duration::from_millis(35)).await;

    // Gate on: the expired lease with a live in-process waiter is untouched.
    let gated = store
        .claim_due_occurrences(claim_request(
            Duration::milliseconds(25),
            [occurrence.occurrence_id.clone()].into_iter().collect(),
        ))
        .await
        .expect("gated claim");
    assert!(
        gated.claimed.is_empty(),
        "an expired lease with a live in-process waiter must not be reclaimed"
    );
    let current = store
        .get_occurrence(&occurrence.occurrence_id)
        .await
        .expect("get occurrence")
        .expect("occurrence should exist");
    assert_eq!(current.phase, OccurrencePhase::AwaitingCompletion);
    assert_eq!(current.attempt_count, 1);
    let receipts = store
        .list_receipts(&occurrence.occurrence_id)
        .await
        .expect("list receipts");
    assert!(
        !receipts
            .iter()
            .any(|receipt| receipt.stage == DeliveryReceiptStage::LeaseExpired),
        "the gate must prevent the lease-expired receipt while the waiter lives"
    );

    // Gate off (post-crash): the reclaim proceeds with attempt+1 and the
    // lease-expired receipt, exactly as before.
    let reclaimed = store
        .claim_due_occurrences(claim_request(Duration::milliseconds(25), BTreeSet::new()))
        .await
        .expect("reclaim");
    assert_eq!(reclaimed.claimed.len(), 1);
    assert_eq!(reclaimed.claimed[0].attempt_count, 2);
    let receipts = store
        .list_receipts(&occurrence.occurrence_id)
        .await
        .expect("list receipts");
    assert!(
        receipts
            .iter()
            .any(|receipt| receipt.stage == DeliveryReceiptStage::LeaseExpired),
        "reclaiming a dead deliverer must still mint the lease-expired receipt"
    );
}

#[tokio::test]
async fn live_waiter_gate_blocks_false_misfire_in_sqlite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");

    let schedule = commit_schedule(&store, "gate-misfire").await;
    // Pending and past the Skip policy's 30s misfire grace: misfire-required.
    let occurrence =
        commit_occurrence_due_at(&store, &schedule, Utc::now() - Duration::seconds(40)).await;

    // Gate on: a misfire-required row with a live in-process waiter is not
    // terminalized (the delivery is still actually running).
    let gated = store
        .claim_due_occurrences(claim_request(
            Duration::seconds(30),
            [occurrence.occurrence_id.clone()].into_iter().collect(),
        ))
        .await
        .expect("gated claim");
    assert!(gated.claimed.is_empty());
    let current = store
        .get_occurrence(&occurrence.occurrence_id)
        .await
        .expect("get occurrence")
        .expect("occurrence should exist");
    assert_eq!(
        current.phase,
        OccurrencePhase::Pending,
        "a live delivery must not be false-misfired while its waiter is registered"
    );

    // Gate off (post-crash): the misfire proceeds as before.
    store
        .claim_due_occurrences(claim_request(Duration::seconds(30), BTreeSet::new()))
        .await
        .expect("misfire claim");
    let misfired = store
        .get_occurrence(&occurrence.occurrence_id)
        .await
        .expect("get occurrence")
        .expect("occurrence should exist");
    assert_eq!(misfired.phase, OccurrencePhase::Misfired);
}
