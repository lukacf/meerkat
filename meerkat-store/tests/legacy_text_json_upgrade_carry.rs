//! Upgrade-carry coverage for JSON payload columns.
//!
//! Meerkat writes JSON payload columns as BLOB, but SQLite affinity keeps
//! whatever type a writer bound, so carried stores written by external hosts
//! can hold the same UTF-8 JSON as TEXT. Every read must accept both physical
//! encodings; one legacy TEXT row must not fail the whole table.

#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{
    ContentInput, Message, Session, SessionId, SessionMeta, SessionStore, UserMessage,
};
use meerkat_schedule::{
    ClaimDueRequest, CreateScheduleRequest, IntervalTriggerSpec, MisfirePolicy,
    MissingTargetPolicy, Occurrence, OccurrenceLifecycleInput, OccurrenceOrdinal, OverlapPolicy,
    Schedule, ScheduleLifecycleInput, ScheduleStore, ScheduledSessionAction, SessionTargetBinding,
    TargetBinding, TriggerSpec,
};
use meerkat_store::{SqliteScheduleStore, SqliteSessionStore, index::SqliteSessionIndex};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::path::Path;

fn sample_schedule_request() -> CreateScheduleRequest {
    CreateScheduleRequest {
        name: Some("legacy-text-upgrade-carry".to_string()),
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

/// Rewrite a BLOB JSON column value as TEXT in place, mimicking a legacy row
/// written by an external host that bound a string instead of bytes.
fn degrade_json_column_to_text(path: &Path, table: &str, json_column: &str) {
    let conn = Connection::open(path).expect("open sqlite");
    let changed = conn
        .execute(
            &format!("UPDATE {table} SET {json_column} = CAST({json_column} AS TEXT)"),
            [],
        )
        .expect("degrade column to TEXT");
    assert!(changed > 0, "expected at least one {table} row to degrade");
    let text_rows: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE typeof({json_column}) = 'text'"),
            [],
            |row| row.get(0),
        )
        .expect("count TEXT rows");
    assert_eq!(
        text_rows as usize, changed,
        "all degraded {table} rows must be TEXT"
    );
}

#[tokio::test]
async fn schedule_store_reads_legacy_text_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open");

    let schedule = commit_sample_schedule(&store).await;
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
        .expect("commit occurrence");

    // Drive one occurrence to a terminal receipt so schedule_receipts has a row.
    let claim = store
        .claim_due_occurrences(ClaimDueRequest {
            owner_id: "legacy-carry".into(),
            limit: 8,
            lease_duration: Duration::minutes(5),
        })
        .await
        .expect("claim due occurrences");
    assert_eq!(
        claim.claimed.len(),
        1,
        "expected the due occurrence claimed"
    );
    let claimed = claim.claimed[0].clone();
    let claim_token = claimed.claim_token();
    let dispatching = store
        .transition_occurrence_if_current(
            &claimed.occurrence_id,
            claimed.attempt_count,
            claim_token,
            OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: None,
                at_utc: Utc::now(),
            },
        )
        .await
        .expect("dispatch transition")
        .expect("occurrence should still be current")
        .0;
    store
        .transition_occurrence_with_receipt_if_current(
            &dispatching.occurrence_id,
            dispatching.attempt_count,
            dispatching.claim_token(),
            OccurrenceLifecycleInput::Complete { at_utc: Utc::now() },
            None,
        )
        .await
        .expect("complete transition")
        .expect("occurrence should still be current");

    let schedules_before = store
        .list_schedules(Default::default())
        .await
        .expect("list schedules before degrade");
    let occurrences_before = store
        .list_occurrences(meerkat_schedule::OccurrenceFilter {
            include_terminal: true,
            ..Default::default()
        })
        .await
        .expect("list occurrences before degrade");
    let receipts_before = store
        .list_receipts(&occurrence.occurrence_id)
        .await
        .expect("list receipts before degrade");
    assert!(
        !receipts_before.is_empty(),
        "terminal occurrence must have a receipt"
    );

    degrade_json_column_to_text(&path, "schedule_schedules", "schedule_json");
    degrade_json_column_to_text(&path, "schedule_occurrences", "occurrence_json");
    degrade_json_column_to_text(&path, "schedule_receipts", "receipt_json");

    // Re-open the store on the degraded database: every read path must carry.
    let store = SqliteScheduleStore::open(&path).expect("re-open");
    let loaded_schedule = store
        .get_schedule(&schedule.schedule_id)
        .await
        .expect("get_schedule over TEXT row")
        .expect("schedule present");
    assert_eq!(loaded_schedule, schedules_before[0]);
    let schedules_after = store
        .list_schedules(Default::default())
        .await
        .expect("list_schedules over TEXT rows");
    assert_eq!(schedules_after, schedules_before);
    let occurrences_after = store
        .list_occurrences(meerkat_schedule::OccurrenceFilter {
            include_terminal: true,
            ..Default::default()
        })
        .await
        .expect("list_occurrences over TEXT rows");
    assert_eq!(occurrences_after, occurrences_before);
    let receipts_after = store
        .list_receipts(&occurrence.occurrence_id)
        .await
        .expect("list_receipts over TEXT rows");
    assert_eq!(receipts_after, receipts_before);
}

#[tokio::test]
async fn schedule_store_claims_and_writes_over_legacy_text_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path).expect("open");

    let schedule = commit_sample_schedule(&store).await;
    let occurrence_write = Occurrence::planned_write_from_schedule(
        &schedule,
        OccurrenceOrdinal(0),
        Utc::now() - Duration::seconds(1),
    )
    .expect("plan occurrence");
    store
        .commit_occurrence_write(occurrence_write)
        .await
        .expect("commit occurrence");

    degrade_json_column_to_text(&path, "schedule_schedules", "schedule_json");
    degrade_json_column_to_text(&path, "schedule_occurrences", "occurrence_json");

    // The claim path joins schedule and occurrence JSON columns and rewrites
    // the claimed row; both must carry over legacy TEXT rows.
    let store = SqliteScheduleStore::open(&path).expect("re-open");
    let claim = store
        .claim_due_occurrences(ClaimDueRequest {
            owner_id: "legacy-carry".into(),
            limit: 8,
            lease_duration: Duration::minutes(5),
        })
        .await
        .expect("claim_due_occurrences over TEXT rows");
    assert_eq!(
        claim.claimed.len(),
        1,
        "expected the due occurrence claimed"
    );

    // Rewrites go through the canonical BLOB write path.
    let conn = Connection::open(&path).expect("open sqlite");
    let claimed_type: String = conn
        .query_row(
            "SELECT typeof(occurrence_json) FROM schedule_occurrences WHERE occurrence_id = ?1",
            [claim.claimed[0].occurrence_id.to_string()],
            |row| row.get(0),
        )
        .expect("claimed row type");
    assert_eq!(claimed_type, "blob", "rewritten rows normalize to BLOB");

    // Authorized schedule writes verify the current row inside the
    // transaction; that read must also carry over a legacy TEXT row.
    let current = store
        .get_schedule(&schedule.schedule_id)
        .await
        .expect("get_schedule over TEXT row")
        .expect("schedule present");
    let pause = Schedule::apply(
        Some(current),
        ScheduleLifecycleInput::Pause { at_utc: Utc::now() },
    )
    .expect("pause should pass generated authority");
    store
        .commit_schedule_write(pause.into_authorized_write())
        .await
        .expect("commit_schedule_write over TEXT current row");
}

#[tokio::test]
async fn session_store_reads_legacy_encodings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("sessions.sqlite3");
    let store = SqliteSessionStore::open(&path).expect("open");

    let mut session = Session::new();
    session.push(Message::User(UserMessage::text(
        "hello from a carried store".to_string(),
    )));
    let id = session.id().clone();
    store.save(&session).await.expect("save session");

    let conn = Connection::open(&path).expect("open sqlite");
    // session_json is canonically BLOB; degrade to TEXT.
    conn.execute(
        "UPDATE sessions SET session_json = CAST(session_json AS TEXT)",
        [],
    )
    .expect("degrade session_json");
    // metadata_json is canonically TEXT; degrade to BLOB (opposite direction).
    conn.execute(
        "UPDATE sessions SET metadata_json = CAST(metadata_json AS BLOB)",
        [],
    )
    .expect("degrade metadata_json");
    drop(conn);

    let store = SqliteSessionStore::open(&path).expect("re-open");
    let loaded = store
        .load(&id)
        .await
        .expect("load over TEXT session_json")
        .expect("session present");
    assert_eq!(loaded.id(), &id);
    assert_eq!(loaded.messages().len(), session.messages().len());
    let listed = store
        .list(Default::default())
        .await
        .expect("list over BLOB metadata_json");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
}

#[tokio::test]
async fn session_index_reads_legacy_text_meta_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("index.sqlite3");
    let index = SqliteSessionIndex::open(&path).expect("open");

    let session = Session::new();
    let id = session.id().clone();
    index
        .insert_meta(SessionMeta {
            id: id.clone(),
            created_at: session.created_at(),
            updated_at: session.updated_at(),
            message_count: session.messages().len(),
            total_tokens: 0,
            metadata: session.metadata().clone(),
        })
        .expect("insert meta");

    degrade_json_column_to_text(&path, "session_index", "meta_json");

    let index = SqliteSessionIndex::open(&path).expect("re-open");
    let looked_up = index
        .lookup_meta(&id)
        .expect("lookup over TEXT meta_json")
        .expect("meta present");
    assert_eq!(looked_up.id, id);
    let listed = index.list_meta(Default::default()).expect("list_meta");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
}
