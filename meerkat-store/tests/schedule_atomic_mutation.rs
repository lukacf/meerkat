#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    CreateScheduleRequest, MisfirePolicy, MissingTargetPolicy, Occurrence, OccurrenceFilter,
    OccurrencePhase, OverlapPolicy, PendingSupersession, Schedule, ScheduleStore,
    ScheduledSessionAction, SessionTargetBinding, TargetBinding, TriggerSpec,
};
use meerkat_store::RedbScheduleStore;
#[cfg(feature = "sqlite")]
use meerkat_store::SqliteScheduleStore;
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
                skill_references: Vec::new(),
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
    );
    store.put_occurrence(old_pending.clone()).await?;

    let mut updated = original.clone();
    updated.revision = updated.revision.next();
    updated.updated_at_utc = Utc::now();
    updated.next_occurrence_ordinal = updated.next_occurrence_ordinal.next();

    let replacement = Occurrence::planned_from_schedule(
        &updated,
        updated.next_occurrence_ordinal,
        Utc::now() + Duration::minutes(10),
    );

    store
        .commit_schedule_mutation(
            updated.clone(),
            vec![replacement.clone()],
            Some(PendingSupersession {
                at_utc: Utc::now(),
                superseded_by_revision: updated.revision,
            }),
        )
        .await?;

    let stored = store
        .get_schedule(&updated.schedule_id)
        .await?
        .expect("updated schedule should exist");
    assert_eq!(stored.revision, updated.revision);

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
async fn redb_atomic_schedule_mutation_supersedes_old_pending()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Arc::new(RedbScheduleStore::open(dir.path().join("schedule.redb"))?)
        as Arc<dyn ScheduleStore>;
    assert_atomic_schedule_mutation_supersedes_old_pending(store).await
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_atomic_schedule_mutation_supersedes_old_pending()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Arc::new(SqliteScheduleStore::open(
        dir.path().join("schedule.sqlite3"),
    )?) as Arc<dyn ScheduleStore>;
    assert_atomic_schedule_mutation_supersedes_old_pending(store).await
}
