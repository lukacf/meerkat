//! Per-row tolerance and scan-bounding contracts for the sqlite schedule
//! store (upstream asks 17–19, HomeCore field regression: one poisoned row —
//! 31/31 occurrences pending across ~5 binary generations — starved every
//! schedule, silently, until operators hand-edited sqlite).
//!
//! - a poisoned occurrence or schedule row is skipped as a TYPED fault and
//!   its healthy neighbors still claim;
//! - terminal rows never enter the claim scan (the SQL prefilter), so a
//!   poisoned terminal row cannot even fault;
//! - the tolerant listing surfaces poisoned schedule rows while the strict
//!   `list_schedules` contract keeps failing wholesale;
//! - legacy Deleted tombstones that persisted a planning cursor heal at the
//!   durable-format parse boundary (upgrade-carry: written under version N,
//!   readable under N+1).

#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, TimeZone, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    ClaimDueRequest, CreateScheduleRequest, IntervalTriggerSpec, MisfirePolicy,
    MissingTargetPolicy, Occurrence, OccurrenceOrdinal, OverlapPolicy, Schedule, ScheduleFilter,
    ScheduleLifecycleInput, ScheduleService, ScheduleStore, ScheduleStoreRowFaultKind,
    ScheduledSessionAction, SessionTargetBinding, TargetBinding, TriggerSpec,
};
use meerkat_store::SqliteScheduleStore;
use rusqlite::{Connection, params};
use std::{collections::BTreeMap, sync::Arc};

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

async fn commit_due_occurrence(store: &SqliteScheduleStore, schedule: &Schedule) -> Occurrence {
    commit_occurrence_at(
        store,
        schedule,
        OccurrenceOrdinal(0),
        Utc::now() - Duration::seconds(1),
    )
    .await
}

async fn commit_occurrence_at(
    store: &SqliteScheduleStore,
    schedule: &Schedule,
    ordinal: OccurrenceOrdinal,
    due_at_utc: chrono::DateTime<Utc>,
) -> Occurrence {
    let write = Occurrence::planned_write_from_schedule(schedule, ordinal, due_at_utc)
        .expect("occurrence planning should pass generated authority");
    let occurrence = write.occurrence().clone();
    store
        .commit_occurrence_write(write)
        .await
        .expect("commit occurrence");
    occurrence
}

fn poison_row(path: &std::path::Path, table: &str, id_column: &str, json_column: &str, id: &str) {
    let conn = Connection::open(path).expect("open sqlite");
    let changed = conn
        .execute(
            &format!("UPDATE {table} SET {json_column} = ?2 WHERE {id_column} = ?1"),
            params![id, b"{not json".as_slice()],
        )
        .expect("poison row");
    assert_eq!(changed, 1, "poisoned row must exist");
}

fn claim_request() -> ClaimDueRequest {
    ClaimDueRequest {
        owner_id: "row-fault-test".to_string(),
        limit: 16,
        lease_duration: Duration::seconds(60),
    }
}

#[tokio::test]
async fn zero_limit_claim_is_a_true_noop() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");
    let schedule = commit_schedule(&store, "zero-limit").await;
    let occurrence = commit_due_occurrence(&store, &schedule).await;

    let result = store
        .claim_due_occurrences(ClaimDueRequest {
            owner_id: "must-not-own".to_string(),
            limit: 0,
            lease_duration: Duration::seconds(60),
        })
        .await
        .expect("zero-limit claim");
    assert!(result.claimed.is_empty());
    assert!(result.row_faults.is_empty());
    let unchanged = store
        .get_occurrence(&occurrence.occurrence_id)
        .await
        .expect("read occurrence")
        .expect("occurrence exists");
    assert_eq!(unchanged.attempt_count, 0);
    assert_eq!(unchanged.claimed_by, None);

    let claimed = store
        .claim_due_occurrences(claim_request())
        .await
        .expect("later non-zero claim");
    assert_eq!(claimed.claimed.len(), 1);
    assert_eq!(claimed.claimed[0].occurrence_id, occurrence.occurrence_id);
}

#[tokio::test]
async fn poisoned_front_page_cannot_permanently_starve_later_due_work() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");
    let base = Utc::now() - Duration::seconds(5);
    let mut poisoned_ids = Vec::new();
    let mut healthy = None;

    // limit=1 has a hard scan budget of 64. Put exactly 64 poisoned rows
    // before one healthy due row so a fixed "LIMIT 64 from the beginning"
    // implementation would starve it forever.
    for index in 0..65_u32 {
        let schedule = commit_schedule(&store, &format!("scan-row-{index}")).await;
        let occurrence = commit_occurrence_at(
            &store,
            &schedule,
            OccurrenceOrdinal(0),
            base + Duration::milliseconds(i64::from(index)),
        )
        .await;
        if index < 64 {
            poisoned_ids.push(occurrence.occurrence_id);
        } else {
            healthy = Some(occurrence);
        }
    }
    {
        let mut conn = Connection::open(&path).expect("open sqlite");
        let tx = conn.transaction().expect("begin poison transaction");
        for occurrence_id in &poisoned_ids {
            tx.execute(
                "UPDATE schedule_occurrences SET occurrence_json = ?2 WHERE occurrence_id = ?1",
                params![occurrence_id.to_string(), b"{not json".as_slice()],
            )
            .expect("poison occurrence");
        }
        tx.commit().expect("commit poison transaction");
    }

    let request = ClaimDueRequest {
        owner_id: "fair-scan".to_string(),
        limit: 1,
        lease_duration: Duration::seconds(60),
    };
    let first = store
        .claim_due_occurrences(request.clone())
        .await
        .expect("first bounded scan");
    assert!(first.claimed.is_empty());
    assert_eq!(first.row_faults.len(), 64);

    let second = store
        .claim_due_occurrences(request)
        .await
        .expect("cursor-advanced scan");
    let healthy = healthy.expect("healthy tail occurrence");
    assert_eq!(
        second
            .claimed
            .iter()
            .map(|occurrence| occurrence.occurrence_id.clone())
            .collect::<Vec<_>>(),
        vec![healthy.occurrence_id],
        "cursor progress across typed row faults must expose later healthy work"
    );
}

#[tokio::test]
async fn claim_scan_skips_poisoned_occurrence_row_and_claims_healthy_neighbor() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");

    let healthy_schedule = commit_schedule(&store, "healthy").await;
    let healthy = commit_due_occurrence(&store, &healthy_schedule).await;
    let poisoned_schedule = commit_schedule(&store, "poisoned-occurrence").await;
    let poisoned = commit_due_occurrence(&store, &poisoned_schedule).await;
    poison_row(
        &path,
        "schedule_occurrences",
        "occurrence_id",
        "occurrence_json",
        &poisoned.occurrence_id.to_string(),
    );

    let result = store
        .claim_due_occurrences(claim_request())
        .await
        .expect("claim must not fail wholesale on a poisoned row");
    assert_eq!(
        result
            .claimed
            .iter()
            .map(|occurrence| occurrence.occurrence_id.clone())
            .collect::<Vec<_>>(),
        vec![healthy.occurrence_id.clone()],
        "the healthy neighbor must still claim"
    );
    assert_eq!(result.row_faults.len(), 1, "the skip must be a typed fault");
    let fault = &result.row_faults[0];
    assert_eq!(fault.kind, ScheduleStoreRowFaultKind::Deserialization);
    assert_eq!(
        fault.occurrence_id.as_deref(),
        Some(poisoned.occurrence_id.to_string().as_str()),
        "the fault must carry the poisoned row's occurrence id"
    );
    assert_eq!(
        fault.schedule_id.as_deref(),
        Some(poisoned_schedule.schedule_id.to_string().as_str()),
        "the fault must carry the owning schedule id"
    );
}

#[tokio::test]
async fn refill_cursor_advances_past_a_poisoned_schedule_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");
    let poisoned = commit_schedule(&store, "poisoned-refill").await;
    let healthy = commit_schedule(&store, "healthy-refill").await;
    {
        let conn = Connection::open(&path).expect("open sqlite");
        conn.execute(
            r"
            UPDATE schedule_schedules
            SET next_refill_at_ms = 1, schedule_json = ?2
            WHERE schedule_id = ?1
            ",
            params![poisoned.schedule_id.to_string(), b"{not json".as_slice()],
        )
        .expect("poison first refill row");
        conn.execute(
            r"
            UPDATE schedule_schedules
            SET next_refill_at_ms = 2
            WHERE schedule_id = ?1
            ",
            params![healthy.schedule_id.to_string()],
        )
        .expect("order healthy refill row after poison");
    }

    let first = store
        .read_due_refill_candidates(1)
        .await
        .expect("first bounded refill page");
    assert!(first.candidates.is_empty());
    assert_eq!(first.row_faults.len(), 1);
    assert_eq!(
        first.row_faults[0].schedule_id.as_deref(),
        Some(poisoned.schedule_id.to_string().as_str())
    );

    let second = store
        .read_due_refill_candidates(1)
        .await
        .expect("cursor-advanced refill page");
    assert_eq!(second.row_faults.len(), 0);
    assert_eq!(second.candidates.len(), 1);
    assert_eq!(
        second.candidates[0].schedule.schedule_id, healthy.schedule_id,
        "poisoned head row must not permanently hide later refill work"
    );
}

#[tokio::test]
async fn sqlite_far_future_schedule_records_exact_refill_wake_without_due_scan() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = Arc::new(SqliteScheduleStore::open(&path).expect("open store"));
    let service = ScheduleService::new(store.clone());
    let start_at_utc = Utc
        .timestamp_millis_opt((Utc::now() + Duration::days(3)).timestamp_millis())
        .single()
        .expect("millisecond timestamp");
    let mut request = sample_schedule_request("far-future-refill");
    request.trigger = TriggerSpec::Interval(IntervalTriggerSpec {
        start_at_utc,
        every_seconds: 60,
        end_at_utc: None,
    });
    request.planning_horizon_days = Some(1);
    request.planning_horizon_occurrences = Some(1);

    let schedule = service.create(request).await.expect("create schedule");
    assert!(
        service
            .list_occurrences(&schedule.schedule_id)
            .await
            .expect("list occurrences")
            .is_empty()
    );
    assert_eq!(
        store
            .next_action_time_utc()
            .await
            .expect("next action")
            .next_action_at_utc,
        Some(start_at_utc - Duration::days(1))
    );
    assert!(
        store
            .read_due_refill_candidates(32)
            .await
            .expect("due refills")
            .candidates
            .is_empty(),
        "ordinary ticks must not revisit a schedule before its exact refill wake"
    );
}

#[tokio::test]
async fn claim_scan_skips_occurrences_of_a_poisoned_schedule_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");

    let healthy_schedule = commit_schedule(&store, "healthy").await;
    let healthy = commit_due_occurrence(&store, &healthy_schedule).await;
    let poisoned_schedule = commit_schedule(&store, "poisoned-schedule").await;
    let _orphaned = commit_due_occurrence(&store, &poisoned_schedule).await;
    poison_row(
        &path,
        "schedule_schedules",
        "schedule_id",
        "schedule_json",
        &poisoned_schedule.schedule_id.to_string(),
    );

    let result = store
        .claim_due_occurrences(claim_request())
        .await
        .expect("claim must not fail wholesale on a poisoned schedule row");
    assert_eq!(
        result
            .claimed
            .iter()
            .map(|occurrence| occurrence.occurrence_id.clone())
            .collect::<Vec<_>>(),
        vec![healthy.occurrence_id.clone()],
    );
    assert_eq!(result.row_faults.len(), 1);
    assert_eq!(
        result.row_faults[0].schedule_id.as_deref(),
        Some(poisoned_schedule.schedule_id.to_string().as_str()),
    );
}

/// Ask 19: terminal rows never enter the claim scan. A poisoned TERMINAL row
/// produces no fault at all — proof the SQL prefilter excluded it before the
/// parse boundary (and that multi-GB terminal history never pays per-tick
/// deserialization).
#[tokio::test]
async fn claim_scan_never_deserializes_terminal_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");

    let schedule = commit_schedule(&store, "with-terminal-history").await;
    let terminal = commit_due_occurrence(&store, &schedule).await;
    {
        // Force the row's PHASE COLUMN terminal, then poison its JSON: only
        // a scan that reads the row would notice.
        let conn = Connection::open(&path).expect("open sqlite");
        conn.execute(
            "UPDATE schedule_occurrences SET phase = 'completed', occurrence_json = ?2 WHERE occurrence_id = ?1",
            params![terminal.occurrence_id.to_string(), b"{not json".as_slice()],
        )
        .expect("terminalize and poison row");
    }
    let healthy_schedule = commit_schedule(&store, "healthy").await;
    let healthy = commit_due_occurrence(&store, &healthy_schedule).await;

    let result = store
        .claim_due_occurrences(claim_request())
        .await
        .expect("claim over terminal poison must succeed");
    assert_eq!(
        result
            .claimed
            .iter()
            .map(|occurrence| occurrence.occurrence_id.clone())
            .collect::<Vec<_>>(),
        vec![healthy.occurrence_id.clone()],
    );
    assert!(
        result.row_faults.is_empty(),
        "a terminal row must never be read, so it can never fault: {:?}",
        result.row_faults
    );
}

/// Ask 19, schedule half of the prefilter: rows of non-active schedules never
/// enter the claim scan. The schedule row's JSON is poisoned so the Rust-side
/// canonical-phase guard cannot be what excludes it — zero faults proves the
/// SQL `s.phase = 'active'` predicate excluded the row before the parse
/// boundary.
#[tokio::test]
async fn claim_scan_never_deserializes_rows_of_non_active_schedules() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");

    let tombstoned_schedule = commit_schedule(&store, "tombstoned").await;
    let _pending_under_tombstone = commit_due_occurrence(&store, &tombstoned_schedule).await;
    {
        // Force the schedule row's PHASE COLUMN non-active, then poison its
        // JSON: only a scan that reads the row would notice.
        let conn = Connection::open(&path).expect("open sqlite");
        let changed = conn
            .execute(
                "UPDATE schedule_schedules SET phase = 'deleted', schedule_json = ?2 WHERE schedule_id = ?1",
                params![
                    tombstoned_schedule.schedule_id.to_string(),
                    b"{not json".as_slice()
                ],
            )
            .expect("tombstone and poison schedule row");
        assert_eq!(changed, 1);
    }
    let healthy_schedule = commit_schedule(&store, "healthy").await;
    let healthy = commit_due_occurrence(&store, &healthy_schedule).await;

    let result = store
        .claim_due_occurrences(claim_request())
        .await
        .expect("claim over a non-active poisoned schedule row must succeed");
    assert_eq!(
        result
            .claimed
            .iter()
            .map(|occurrence| occurrence.occurrence_id.clone())
            .collect::<Vec<_>>(),
        vec![healthy.occurrence_id.clone()],
    );
    assert!(
        result.row_faults.is_empty(),
        "a non-active schedule's rows must never be read, so they can never fault: {:?}",
        result.row_faults
    );
}

/// A store WRITE failure inside the per-row misfire arm must abort the whole
/// claim transaction (nothing commits), NOT degrade into a per-row fault: a
/// committed misfire receipt without the terminalized occurrence row would be
/// an intra-row split commit.
#[tokio::test]
async fn claim_misfire_store_write_failure_aborts_the_whole_claim() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");

    let schedule = commit_schedule(&store, "misfire-required").await;
    // Due far beyond the 3600s catch-up window → MisfireRequired.
    let write = Occurrence::planned_write_from_schedule(
        &schedule,
        OccurrenceOrdinal(0),
        Utc::now() - Duration::hours(2),
    )
    .expect("occurrence planning should pass generated authority");
    let misfired = write.occurrence().clone();
    store
        .commit_occurrence_write(write)
        .await
        .expect("commit occurrence");

    {
        // Deterministic write-failure injection: receipts INSERTs abort.
        let conn = Connection::open(&path).expect("open sqlite");
        conn.execute_batch(
            "CREATE TRIGGER synthetic_receipt_write_failure
             BEFORE INSERT ON schedule_receipts
             BEGIN SELECT RAISE(ABORT, 'synthetic receipt write failure'); END;",
        )
        .expect("install failing trigger");
    }

    let error = store
        .claim_due_occurrences(claim_request())
        .await
        .expect_err("a store write failure must abort the claim wholesale");
    assert!(
        error
            .to_string()
            .contains("synthetic receipt write failure"),
        "the abort must carry the store failure, got: {error}"
    );

    // Nothing committed: the occurrence row was not terminalized.
    let conn = Connection::open(&path).expect("open sqlite");
    let phase: String = conn
        .query_row(
            "SELECT phase FROM schedule_occurrences WHERE occurrence_id = ?1",
            params![misfired.occurrence_id.to_string()],
            |row| row.get(0),
        )
        .expect("occurrence row present");
    assert_eq!(
        phase, "pending",
        "the transaction must roll back the misfire terminalization"
    );
    let receipts: i64 = conn
        .query_row("SELECT COUNT(*) FROM schedule_receipts", [], |row| {
            row.get(0)
        })
        .expect("count receipts");
    assert_eq!(receipts, 0, "no orphan receipt may commit");
}

#[tokio::test]
async fn tolerant_listing_surfaces_poisoned_schedule_rows_and_strict_listing_stays_strict() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");

    let healthy = commit_schedule(&store, "healthy").await;
    let poisoned = commit_schedule(&store, "poisoned").await;
    poison_row(
        &path,
        "schedule_schedules",
        "schedule_id",
        "schedule_json",
        &poisoned.schedule_id.to_string(),
    );

    let (schedules, faults) = store
        .list_schedules_with_row_faults(ScheduleFilter::default())
        .await
        .expect("tolerant listing must survive a poisoned row");
    assert_eq!(
        schedules
            .iter()
            .map(|schedule| schedule.schedule_id.clone())
            .collect::<Vec<_>>(),
        vec![healthy.schedule_id.clone()],
    );
    assert_eq!(faults.len(), 1);
    assert_eq!(
        faults[0].schedule_id.as_deref(),
        Some(poisoned.schedule_id.to_string().as_str()),
    );
    assert!(faults[0].occurrence_id.is_none());

    // The strict trait contract is unchanged: whole-listing failure.
    store
        .list_schedules(ScheduleFilter::default())
        .await
        .expect_err("strict listing keeps failing wholesale");
}

/// Ask 18 upgrade-carry: a Deleted tombstone written by a binary generation
/// whose Delete transition did not clear the planning cursor must heal at
/// the durable-format parse boundary — `list()` (strict!) and `get` succeed
/// and the healed tombstone carries no cursor. HomeCore needed manual sqlite
/// surgery on 16 such rows before this.
#[tokio::test]
async fn legacy_deleted_tombstone_with_planning_cursor_heals_on_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open store");

    let schedule = commit_schedule(&store, "to-delete").await;
    let deleted = Schedule::apply(
        Some(schedule),
        ScheduleLifecycleInput::Delete { at_utc: Utc::now() },
    )
    .expect("delete should pass generated authority");
    let tombstone = deleted.schedule.clone();
    store
        .commit_schedule_mutation(deleted.into_authorized_write(), Vec::new())
        .await
        .expect("commit tombstone");

    // Re-shape the persisted row into the legacy form: planning cursor
    // retained on BOTH the machine state and the projection.
    {
        let conn = Connection::open(&path).expect("open sqlite");
        let json: Vec<u8> = conn
            .query_row(
                "SELECT schedule_json FROM schedule_schedules WHERE schedule_id = ?1",
                params![tombstone.schedule_id.to_string()],
                |row| row.get(0),
            )
            .expect("select tombstone json");
        let mut value = serde_json::from_slice::<serde_json::Value>(&json).expect("parse json");
        value["machine_state"]["planning_cursor_utc_ms"] = serde_json::json!(1_700_000_000_000u64);
        value["planning_cursor_utc"] = serde_json::json!("2023-11-14T22:13:20Z");
        let updated = serde_json::to_vec(&value).expect("serialize legacy tombstone");
        conn.execute(
            "UPDATE schedule_schedules SET schedule_json = ?2 WHERE schedule_id = ?1",
            params![tombstone.schedule_id.to_string(), updated],
        )
        .expect("write legacy tombstone");
    }

    // The STRICT listing must succeed over the legacy tombstone.
    let listed = store
        .list_schedules(ScheduleFilter {
            include_deleted: true,
            ..ScheduleFilter::default()
        })
        .await
        .expect("list over a legacy tombstone must succeed");
    let healed = listed
        .iter()
        .find(|candidate| candidate.schedule_id == tombstone.schedule_id)
        .expect("tombstone listed");
    assert!(
        healed.planning_cursor_utc.is_none(),
        "healing must clear the projection cursor"
    );
    let reserialized = serde_json::to_value(healed).expect("healed tombstone reserializes");
    assert!(
        reserialized["machine_state"]["planning_cursor_utc_ms"].is_null(),
        "healing must clear the machine-state cursor: {reserialized}"
    );

    let fetched = store
        .get_schedule(&tombstone.schedule_id)
        .await
        .expect("get over a legacy tombstone must succeed")
        .expect("tombstone present");
    assert!(fetched.planning_cursor_utc.is_none());
}
