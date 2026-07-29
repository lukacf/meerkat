//! Equivalence contract for the occurrence-listing SQL pushdown (2026-07-29
//! incident: `list_occurrences` selected every row ever written and filtered
//! in Rust, so accumulated terminal history made every call O(all rows) — a
//! past deployment's operator remedy was wiping the tables).
//!
//! The pushed-down query must return exactly what the old full-scan +
//! Rust-filter path returned, for every filter shape, over a store seeded
//! with live AND terminal rows. The reference below IS the old Rust chain,
//! applied to the unfiltered canonical-order listing.

#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    ClaimDueRequest, CreateScheduleRequest, IntervalTriggerSpec, MisfirePolicy,
    MissingTargetPolicy, Occurrence, OccurrenceFilter, OccurrenceOrdinal, OccurrencePhase,
    OverlapPolicy, Schedule, ScheduleLifecycleInput, ScheduleStore, ScheduledSessionAction,
    SessionTargetBinding, TargetBinding, TriggerSpec,
};
use meerkat_store::SqliteScheduleStore;
use rusqlite::{Connection, params};
use std::collections::BTreeMap;

fn sample_schedule_request(name: &str) -> CreateScheduleRequest {
    CreateScheduleRequest {
        name: Some(name.to_string()),
        description: None,
        trigger: TriggerSpec::Interval(IntervalTriggerSpec {
            start_at_utc: Utc::now(),
            every_seconds: 60,
            end_at_utc: None,
        }),
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
        misfire_policy: MisfirePolicy::CatchUpWithin {
            window_seconds: 3600,
        },
        overlap_policy: OverlapPolicy::AllowConcurrent,
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

async fn commit_occurrence(
    store: &SqliteScheduleStore,
    schedule: &Schedule,
    ordinal: u64,
    due_at_utc: chrono::DateTime<Utc>,
) -> Occurrence {
    let write =
        Occurrence::planned_write_from_schedule(schedule, OccurrenceOrdinal(ordinal), due_at_utc)
            .expect("occurrence planning should pass generated authority");
    let occurrence = write.occurrence().clone();
    store
        .commit_occurrence_write(write)
        .await
        .expect("commit occurrence");
    occurrence
}

/// The exact filter chain `list_occurrences_impl` ran BEFORE the SQL
/// pushdown, over the full canonical-order listing. Any divergence between
/// this and the pushed-down store call is a pushdown bug.
fn reference_filter(all: &[Occurrence], filter: &OccurrenceFilter) -> Vec<Occurrence> {
    let mut occurrences = Vec::new();
    for occurrence in all {
        if !filter.include_terminal && occurrence.is_terminal() {
            continue;
        }
        if filter
            .schedule_id
            .as_ref()
            .is_some_and(|schedule_id| &occurrence.schedule_id != schedule_id)
        {
            continue;
        }
        if filter.phase.is_some_and(|phase| occurrence.phase != phase) {
            continue;
        }
        if filter
            .due_after_utc
            .is_some_and(|due_after| occurrence.due_at_utc < due_after)
        {
            continue;
        }
        if filter
            .due_before_utc
            .is_some_and(|due_before| occurrence.due_at_utc > due_before)
        {
            continue;
        }
        occurrences.push(occurrence.clone());
        if filter.limit.is_some_and(|limit| occurrences.len() >= limit) {
            break;
        }
    }
    occurrences
}

fn ids(occurrences: &[Occurrence]) -> Vec<String> {
    occurrences
        .iter()
        .map(|occurrence| occurrence.occurrence_id.to_string())
        .collect()
}

/// Seed one store with a spread of phases (Pending, Claimed, Misfired) and
/// due times across two schedules, and hand back the schedules.
async fn seed_store(store: &SqliteScheduleStore) -> (Schedule, Schedule) {
    let schedule_a = commit_schedule(store, "schedule-a").await;
    // Due now → claimable; due 2h ago with a 3600s catch-up window →
    // MisfireRequired → terminal Misfired row. Both realized by the claim
    // call below through the machine-owned due classification.
    commit_occurrence(store, &schedule_a, 0, Utc::now() - Duration::seconds(1)).await;
    commit_occurrence(store, &schedule_a, 1, Utc::now() - Duration::hours(2)).await;
    let claimed = store
        .claim_due_occurrences(ClaimDueRequest {
            owner_id: "pushdown-test".to_string(),
            limit: 16,
            lease_duration: Duration::seconds(60),
        })
        .await
        .expect("claim");
    assert_eq!(claimed.claimed.len(), 1, "one row claims, one misfires");
    assert!(claimed.row_faults.is_empty(), "{:?}", claimed.row_faults);
    // Committed after the claim call so they stay Pending despite one being
    // already due.
    commit_occurrence(store, &schedule_a, 2, Utc::now() + Duration::minutes(5)).await;
    let schedule_b = commit_schedule(store, "schedule-b").await;
    commit_occurrence(store, &schedule_b, 0, Utc::now() - Duration::seconds(30)).await;
    (schedule_a, schedule_b)
}

#[tokio::test]
async fn pushed_down_listing_matches_the_reference_filter_chain() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteScheduleStore::open(dir.path().join("schedule.sqlite3")).expect("open store");
    let (schedule_a, schedule_b) = seed_store(&store).await;

    // The unfiltered listing is the reference corpus: no predicate is
    // pushed for it, so it is the old path's full canonical-order scan.
    let all = store
        .list_occurrences(OccurrenceFilter {
            include_terminal: true,
            ..OccurrenceFilter::default()
        })
        .await
        .expect("list all");
    assert_eq!(all.len(), 4, "seed must produce 4 rows");
    let phases: Vec<OccurrencePhase> = all.iter().map(|occurrence| occurrence.phase).collect();
    assert!(phases.contains(&OccurrencePhase::Claimed), "{phases:?}");
    assert!(phases.contains(&OccurrencePhase::Misfired), "{phases:?}");
    assert!(phases.contains(&OccurrencePhase::Pending), "{phases:?}");

    let now = Utc::now();
    let filters = vec![
        OccurrenceFilter::default(),
        OccurrenceFilter {
            include_terminal: true,
            ..OccurrenceFilter::default()
        },
        OccurrenceFilter {
            schedule_id: Some(schedule_a.schedule_id.clone()),
            include_terminal: true,
            ..OccurrenceFilter::default()
        },
        // The per-tick hot path (planner + session-binding sweeps).
        OccurrenceFilter {
            schedule_id: Some(schedule_a.schedule_id.clone()),
            phase: Some(OccurrencePhase::Pending),
            ..OccurrenceFilter::default()
        },
        OccurrenceFilter {
            schedule_id: Some(schedule_b.schedule_id.clone()),
            ..OccurrenceFilter::default()
        },
        OccurrenceFilter {
            phase: Some(OccurrencePhase::Claimed),
            include_terminal: true,
            ..OccurrenceFilter::default()
        },
        OccurrenceFilter {
            phase: Some(OccurrencePhase::Misfired),
            include_terminal: true,
            ..OccurrenceFilter::default()
        },
        // Terminal-phase filter under terminal exclusion: must be empty.
        OccurrenceFilter {
            phase: Some(OccurrencePhase::Misfired),
            ..OccurrenceFilter::default()
        },
        OccurrenceFilter {
            due_after_utc: Some(now - Duration::minutes(1)),
            include_terminal: true,
            ..OccurrenceFilter::default()
        },
        OccurrenceFilter {
            due_before_utc: Some(now),
            ..OccurrenceFilter::default()
        },
        OccurrenceFilter {
            due_after_utc: Some(now - Duration::hours(3)),
            due_before_utc: Some(now + Duration::minutes(1)),
            include_terminal: true,
            ..OccurrenceFilter::default()
        },
        // Pushed LIMIT (every active predicate exact in SQL).
        OccurrenceFilter {
            include_terminal: true,
            limit: Some(1),
            ..OccurrenceFilter::default()
        },
        OccurrenceFilter {
            include_terminal: true,
            limit: Some(2),
            ..OccurrenceFilter::default()
        },
        OccurrenceFilter {
            schedule_id: Some(schedule_a.schedule_id.clone()),
            include_terminal: true,
            limit: Some(2),
            ..OccurrenceFilter::default()
        },
        // Unpushed LIMIT (terminal exclusion active): Rust-side bound.
        OccurrenceFilter {
            limit: Some(1),
            ..OccurrenceFilter::default()
        },
        OccurrenceFilter {
            due_after_utc: Some(now - Duration::hours(3)),
            include_terminal: true,
            limit: Some(2),
            ..OccurrenceFilter::default()
        },
    ];

    for filter in filters {
        let listed = store
            .list_occurrences(filter.clone())
            .await
            .expect("filtered listing");
        assert_eq!(
            ids(&listed),
            ids(&reference_filter(&all, &filter)),
            "pushdown diverged from the reference chain for {filter:?}"
        );
    }
}

/// The bounded-scan payoff, pinned: with terminal rows excluded in SQL, a
/// poisoned TERMINAL row never reaches the parse boundary, so the live
/// listing survives it (the incident deployments accumulated exactly such
/// history). A listing that ADMITS the row (`include_terminal: true`) keeps
/// the old strict behavior and fails typed.
#[tokio::test]
async fn live_listing_survives_poisoned_terminal_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");
    let (_schedule_a, _schedule_b) = seed_store(&store).await;

    let all = store
        .list_occurrences(OccurrenceFilter {
            include_terminal: true,
            ..OccurrenceFilter::default()
        })
        .await
        .expect("list all");
    let terminal = all
        .iter()
        .find(|occurrence| occurrence.phase == OccurrencePhase::Misfired)
        .expect("seed produced a terminal row");
    let live_ids: Vec<String> = all
        .iter()
        .filter(|occurrence| !occurrence.is_terminal())
        .map(|occurrence| occurrence.occurrence_id.to_string())
        .collect();

    {
        let conn = Connection::open(&path).expect("open sqlite");
        let changed = conn
            .execute(
                "UPDATE schedule_occurrences SET occurrence_json = ?2 WHERE occurrence_id = ?1",
                params![terminal.occurrence_id.to_string(), b"{not json".as_slice()],
            )
            .expect("poison terminal row");
        assert_eq!(changed, 1);
    }

    let live = store
        .list_occurrences(OccurrenceFilter::default())
        .await
        .expect("live listing must never read excluded terminal rows");
    assert_eq!(ids(&live), live_ids);

    store
        .list_occurrences(OccurrenceFilter {
            include_terminal: true,
            ..OccurrenceFilter::default()
        })
        .await
        .expect_err("a listing that admits the poisoned row keeps failing typed");
}
