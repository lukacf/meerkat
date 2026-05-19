#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    CreateScheduleRequest, IntervalTriggerSpec, MisfirePolicy, MissingTargetPolicy, Occurrence,
    OccurrenceOrdinal, OccurrencePhase, OverlapPolicy, Schedule, SchedulePhase, ScheduleStore,
    ScheduledSessionAction, SessionTargetBinding, TargetBinding, TriggerSpec,
};
use meerkat_store::SqliteScheduleStore;
use rusqlite::{Connection, params};
use std::collections::BTreeMap;

fn sample_schedule() -> Schedule {
    Schedule::new(CreateScheduleRequest {
        name: Some("machine-state-ratchet".to_string()),
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
        misfire_policy: MisfirePolicy::Skip,
        overlap_policy: OverlapPolicy::SkipIfRunning,
        missing_target_policy: MissingTargetPolicy::MarkMisfired,
        labels: BTreeMap::new(),
        planning_horizon_days: Some(1),
        planning_horizon_occurrences: Some(2),
    })
    .expect("sample schedule creation should pass generated authority")
}

fn corrupt_json_blob(
    path: &std::path::Path,
    table: &str,
    id_column: &str,
    json_column: &str,
    id: &str,
    mutate: impl FnOnce(&mut serde_json::Value),
) {
    let conn = Connection::open(path).expect("open sqlite");
    let json: Vec<u8> = conn
        .query_row(
            &format!("SELECT {json_column} FROM {table} WHERE {id_column} = ?1"),
            params![id],
            |row| row.get(0),
        )
        .expect("select json");
    let mut value = serde_json::from_slice::<serde_json::Value>(&json).expect("parse json");
    mutate(&mut value);
    let updated = serde_json::to_vec(&value).expect("serialize json");
    conn.execute(
        &format!("UPDATE {table} SET {json_column} = ?2 WHERE {id_column} = ?1"),
        params![id, updated],
    )
    .expect("update json");
}

#[tokio::test]
async fn sqlite_schedule_without_machine_state_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open");
    let schedule = sample_schedule();
    store
        .put_schedule(schedule.clone())
        .await
        .expect("put schedule");

    corrupt_json_blob(
        &path,
        "schedule_schedules",
        "schedule_id",
        "schedule_json",
        &schedule.schedule_id.to_string(),
        |value| {
            value
                .as_object_mut()
                .expect("schedule json object")
                .remove("machine_state");
        },
    );

    let error = store
        .get_schedule(&schedule.schedule_id)
        .await
        .expect_err("missing machine_state must not be recovered from projections");
    assert!(error.to_string().contains("machine_state"));
}

#[tokio::test]
async fn sqlite_occurrence_with_mismatched_phase_projection_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open");
    let schedule = sample_schedule();
    store
        .put_schedule(schedule.clone())
        .await
        .expect("put schedule");
    let occurrence = Occurrence::planned_from_schedule(&schedule, OccurrenceOrdinal(0), Utc::now())
        .expect("plan occurrence");
    store
        .put_occurrence(occurrence.clone())
        .await
        .expect("put occurrence");

    corrupt_json_blob(
        &path,
        "schedule_occurrences",
        "occurrence_id",
        "occurrence_json",
        &occurrence.occurrence_id.to_string(),
        |value| {
            value
                .as_object_mut()
                .expect("occurrence json object")
                .insert("phase".to_string(), serde_json::json!("completed"));
        },
    );

    let error = store
        .get_occurrence(&occurrence.occurrence_id)
        .await
        .expect_err("phase projection must not override machine_state");
    assert!(error.to_string().contains("phase projection"));
}

#[tokio::test]
async fn sqlite_store_rejects_projection_mutation_without_machine_authority() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open");

    let mut schedule = sample_schedule();
    schedule.phase = SchedulePhase::Paused;
    let error = store
        .put_schedule(schedule)
        .await
        .expect_err("mutated schedule projection must fail closed");
    assert!(error.to_string().contains("projection"));

    let schedule = sample_schedule();
    store
        .put_schedule(schedule.clone())
        .await
        .expect("put schedule");
    let mut occurrence = Occurrence::planned_from_schedule(
        &schedule,
        OccurrenceOrdinal(0),
        Utc::now() + Duration::seconds(1),
    )
    .expect("plan occurrence");
    occurrence.phase = OccurrencePhase::Completed;
    let error = store
        .put_occurrence(occurrence)
        .await
        .expect_err("mutated occurrence projection must fail closed");
    assert!(error.to_string().contains("projection"));
}
