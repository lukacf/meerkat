#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    CreateScheduleRequest, IntervalTriggerSpec, MisfirePolicy, MissingTargetPolicy, Occurrence,
    OccurrenceOrdinal, OverlapPolicy, Schedule, ScheduleLifecycleInput, ScheduleStore,
    ScheduledSessionAction, SessionTargetBinding, TargetBinding, TriggerSpec,
};
use meerkat_store::SqliteScheduleStore;
use rusqlite::{Connection, params};
use std::collections::BTreeMap;

fn sample_schedule_request() -> CreateScheduleRequest {
    CreateScheduleRequest {
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
    }
}

async fn commit_sample_schedule(store: &SqliteScheduleStore) -> Schedule {
    let mutator = Schedule::apply(
        None,
        ScheduleLifecycleInput::Create(sample_schedule_request()),
    )
    .expect("sample schedule creation should pass generated authority");
    let schedule = mutator.schedule.clone();
    store
        .commit_schedule_write(mutator.into_authorized_write())
        .await
        .expect("commit schedule");
    schedule
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
    let schedule = commit_sample_schedule(&store).await;

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
async fn sqlite_schedule_machine_schema_version_mismatch_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open");
    let schedule = commit_sample_schedule(&store).await;

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
                .get_mut("machine_state")
                .and_then(serde_json::Value::as_object_mut)
                .expect("schedule machine_state object")
                .insert("schema_version".to_string(), serde_json::json!(0));
        },
    );

    let error = store
        .get_schedule(&schedule.schedule_id)
        .await
        .expect_err("schema version mismatch must fail closed");
    assert!(error.to_string().contains("schema_version"));
}

#[tokio::test]
async fn sqlite_schedule_with_unrecoverable_machine_state_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open");
    let schedule = commit_sample_schedule(&store).await;
    let cursor = Utc::now();
    let cursor_ms = u64::try_from(cursor.timestamp_millis()).expect("positive cursor millis");

    corrupt_json_blob(
        &path,
        "schedule_schedules",
        "schedule_id",
        "schedule_json",
        &schedule.schedule_id.to_string(),
        |value| {
            let object = value.as_object_mut().expect("schedule json object");
            object.insert("phase".to_string(), serde_json::json!("deleted"));
            object.insert(
                "planning_cursor_utc".to_string(),
                serde_json::to_value(cursor).expect("cursor json"),
            );
            let machine = object
                .get_mut("machine_state")
                .and_then(serde_json::Value::as_object_mut)
                .expect("schedule machine_state object");
            machine.insert("lifecycle_phase".to_string(), serde_json::json!("deleted"));
            machine.insert(
                "planning_cursor_utc_ms".to_string(),
                serde_json::json!(cursor_ms),
            );
        },
    );

    let error = store
        .get_schedule(&schedule.schedule_id)
        .await
        .expect_err("generated recovery invariants must fail closed");
    assert!(
        error
            .to_string()
            .contains("generated ScheduleLifecycleMachine rejected")
    );
}

#[tokio::test]
async fn sqlite_occurrence_with_mismatched_phase_projection_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open");
    let schedule = commit_sample_schedule(&store).await;
    let occurrence_write =
        Occurrence::planned_write_from_schedule(&schedule, OccurrenceOrdinal(0), Utc::now())
            .expect("plan occurrence");
    let occurrence = occurrence_write.occurrence().clone();
    store
        .commit_occurrence_write(occurrence_write)
        .await
        .expect("commit occurrence");

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
async fn sqlite_occurrence_with_unrecoverable_machine_state_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open");
    let schedule = commit_sample_schedule(&store).await;
    let occurrence_write =
        Occurrence::planned_write_from_schedule(&schedule, OccurrenceOrdinal(0), Utc::now())
            .expect("plan occurrence");
    let occurrence = occurrence_write.occurrence().clone();
    store
        .commit_occurrence_write(occurrence_write)
        .await
        .expect("commit occurrence");

    corrupt_json_blob(
        &path,
        "schedule_occurrences",
        "occurrence_id",
        "occurrence_json",
        &occurrence.occurrence_id.to_string(),
        |value| {
            let object = value.as_object_mut().expect("occurrence json object");
            object.insert("phase".to_string(), serde_json::json!("delivery_failed"));
            let machine = object
                .get_mut("machine_state")
                .and_then(serde_json::Value::as_object_mut)
                .expect("occurrence machine_state object");
            machine.insert(
                "lifecycle_phase".to_string(),
                serde_json::json!("delivery_failed"),
            );
        },
    );

    let error = store
        .get_occurrence(&occurrence.occurrence_id)
        .await
        .expect_err("generated occurrence recovery invariants must fail closed");
    assert!(
        error
            .to_string()
            .contains("generated OccurrenceLifecycleMachine rejected")
    );
}

#[tokio::test]
async fn sqlite_store_rejects_stale_generated_writes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open");

    let schedule = commit_sample_schedule(&store).await;
    let first_pause = Schedule::apply(
        Some(schedule.clone()),
        ScheduleLifecycleInput::Pause { at_utc: Utc::now() },
    )
    .expect("pause should pass generated authority");
    let stale_pause = Schedule::apply(
        Some(schedule.clone()),
        ScheduleLifecycleInput::Pause { at_utc: Utc::now() },
    )
    .expect("second pause from original should pass generated authority before durability CAS");
    store
        .commit_schedule_write(first_pause.into_authorized_write())
        .await
        .expect("first pause commit should pass");
    let error = store
        .commit_schedule_write(stale_pause.into_authorized_write())
        .await
        .expect_err("stale schedule authority must fail closed");
    assert!(error.to_string().contains("precondition"));

    let occurrence_write = Occurrence::planned_write_from_schedule(
        &schedule,
        OccurrenceOrdinal(0),
        Utc::now() - Duration::seconds(1),
    )
    .expect("plan occurrence");
    let occurrence = occurrence_write.occurrence().clone();
    store
        .commit_occurrence_write(occurrence_write)
        .await
        .expect("commit planned occurrence");
    let first_claim = occurrence
        .clone()
        .apply(meerkat_schedule::OccurrenceLifecycleInput::Claim {
            owner_id: "first-claimer".into(),
            at_utc: Utc::now(),
            lease_expires_at_utc: Utc::now() + Duration::minutes(5),
            claim_token: uuid::Uuid::now_v7(),
        })
        .expect("first claim should pass generated authority");
    let stale_claim = occurrence
        .apply(meerkat_schedule::OccurrenceLifecycleInput::Claim {
            owner_id: "stale-claimer".into(),
            at_utc: Utc::now(),
            lease_expires_at_utc: Utc::now() + Duration::minutes(5),
            claim_token: uuid::Uuid::now_v7(),
        })
        .expect("stale claim should pass before durability CAS");
    store
        .commit_occurrence_write(first_claim.into_authorized_write())
        .await
        .expect("first claim commit should pass");
    let error = store
        .commit_occurrence_write(stale_claim.into_authorized_write())
        .await
        .expect_err("stale occurrence authority must fail closed");
    assert!(error.to_string().contains("precondition"));
}
