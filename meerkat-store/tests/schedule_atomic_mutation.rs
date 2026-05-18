#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    ClaimDueRequest, CreateScheduleRequest, MisfirePolicy, MissingTargetPolicy, Occurrence,
    OccurrenceFilter, OccurrenceOrdinal, OccurrencePhase, OverlapPolicy, PendingSupersession,
    Schedule, ScheduleLifecycleInput, ScheduleStore, ScheduledSessionAction, SessionTargetBinding,
    TargetBinding, TriggerSpec, UpdateScheduleRequest,
};
use meerkat_store::SqliteScheduleStore;
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::sync::Arc;

fn sample_schedule() -> Schedule {
    Schedule::new(CreateScheduleRequest {
        name: Some("atomic-mutation".to_string()),
        description: None,
        trigger: TriggerSpec::Once {
            due_at_utc: Utc::now() + Duration::minutes(5),
        },
        target: TargetBinding::session(SessionTargetBinding::ResumableSession {
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

async fn assert_atomic_schedule_mutation_supersedes_old_pending(
    store: Arc<dyn ScheduleStore>,
) -> Result<(), Box<dyn std::error::Error>> {
    let original = sample_schedule();
    store.put_schedule(original.clone()).await?;

    let old_pending = Occurrence::planned_from_schedule(
        &original,
        original.next_occurrence_ordinal,
        Utc::now() + Duration::minutes(6),
    )
    .expect("old occurrence planning should pass generated authority");
    store.put_occurrence(old_pending.clone()).await?;

    let updated_trigger = TriggerSpec::Once {
        due_at_utc: Utc::now() + Duration::minutes(10),
    };
    let update_mutator = Schedule::apply(
        Some(original.clone()),
        ScheduleLifecycleInput::Update {
            request: UpdateScheduleRequest {
                expected_revision: Some(original.revision),
                trigger: Some(updated_trigger),
                ..UpdateScheduleRequest::default()
            },
            at_utc: Utc::now(),
        },
    )
    .expect("schedule update should pass generated authority");
    let supersession = update_mutator
        .effects
        .iter()
        .find_map(PendingSupersession::from_schedule_effect);
    let mut updated = update_mutator.into_schedule();
    updated = Schedule::apply(
        Some(updated),
        ScheduleLifecycleInput::RecordPlanningWindow {
            planning_cursor_utc: Utc::now() + Duration::minutes(6),
            next_occurrence_ordinal: original.next_occurrence_ordinal.next(),
        },
    )
    .expect("planning window should pass generated authority")
    .into_schedule();

    let replacement = Occurrence::planned_from_schedule(
        &updated,
        updated.next_occurrence_ordinal,
        Utc::now() + Duration::minutes(10),
    )
    .expect("replacement occurrence planning should pass generated authority");

    let committed = store
        .commit_schedule_mutation(updated.clone(), vec![replacement.clone()], supersession)
        .await?;

    let stored = store
        .get_schedule(&updated.schedule_id)
        .await?
        .expect("updated schedule should exist");
    assert_eq!(stored.revision, updated.revision);
    assert!(
        committed
            .superseded_ack_ids
            .contains(&old_pending.occurrence_id)
            && stored
                .superseded_ack_ids
                .contains(&old_pending.occurrence_id),
        "supersession ack should be routed back through schedule authority in the atomic mutation"
    );

    let occurrences = store
        .list_occurrences(OccurrenceFilter {
            schedule_id: Some(updated.schedule_id.clone()),
            include_terminal: true,
            ..OccurrenceFilter::default()
        })
        .await?;

    assert!(
        occurrences.iter().any(|occurrence| {
            occurrence.occurrence_id == old_pending.occurrence_id
                && occurrence.phase == OccurrencePhase::Superseded
                && occurrence.superseded_by_revision == Some(updated.revision)
        }),
        "older pending occurrence should be superseded inside the atomic mutation"
    );
    assert!(
        occurrences.iter().any(|occurrence| {
            occurrence.occurrence_id == replacement.occurrence_id
                && occurrence.phase == OccurrencePhase::Pending
                && occurrence.schedule_revision == updated.revision
        }),
        "replacement occurrence should be present after the atomic mutation"
    );

    Ok(())
}

#[tokio::test]
async fn sqlite_atomic_schedule_mutation_supersedes_old_pending()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Arc::new(SqliteScheduleStore::open(
        dir.path().join("schedule.sqlite3"),
    )?) as Arc<dyn ScheduleStore>;
    assert_atomic_schedule_mutation_supersedes_old_pending(store).await
}

#[tokio::test]
async fn sqlite_claim_due_reads_canonical_schedule_phase() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path)?;

    let schedule = sample_schedule();
    store.put_schedule(schedule.clone()).await?;
    let occurrence = Occurrence::planned_from_schedule(
        &schedule,
        OccurrenceOrdinal(0),
        Utc::now() - Duration::seconds(1),
    )
    .expect("due occurrence planning should pass generated authority");
    store.put_occurrence(occurrence.clone()).await?;

    let conn = Connection::open(&path)?;
    conn.execute(
        "UPDATE schedule_schedules SET phase = 'deleted' WHERE schedule_id = ?1",
        [schedule.schedule_id.to_string()],
    )?;
    drop(conn);

    let claimed = store
        .claim_due_occurrences(ClaimDueRequest {
            owner_id: "sqlite-canonical-phase-test".to_string(),
            limit: 1,
            lease_duration: Duration::minutes(5),
        })
        .await?;

    assert_eq!(claimed.claimed.len(), 1);
    assert_eq!(claimed.claimed[0].occurrence_id, occurrence.occurrence_id);
    Ok(())
}
