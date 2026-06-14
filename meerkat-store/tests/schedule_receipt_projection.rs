#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    CreateScheduleRequest, MisfirePolicy, MissingTargetPolicy, Occurrence, OccurrenceFilter,
    OccurrenceLifecycleInput, OccurrenceOrdinal, OverlapPolicy, Schedule, ScheduleLifecycleInput,
    ScheduleLifecycleMutator, ScheduleStore, ScheduledSessionAction, SessionTargetBinding,
    TargetBinding, TriggerSpec,
};
use meerkat_store::SqliteScheduleStore;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

fn sample_schedule_request() -> CreateScheduleRequest {
    CreateScheduleRequest {
        name: Some("receipt-projection".to_string()),
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
        planning_horizon_occurrences: Some(1),
    }
}

fn sample_schedule_mutator() -> ScheduleLifecycleMutator {
    Schedule::apply(
        None,
        ScheduleLifecycleInput::Create(sample_schedule_request()),
    )
    .expect("sample schedule creation should pass generated authority")
}

async fn sample_in_flight_occurrence(
    store: &dyn ScheduleStore,
    schedule: &Schedule,
) -> Result<Occurrence, Box<dyn std::error::Error>> {
    let occurrence_write = Occurrence::planned_write_from_schedule(
        schedule,
        OccurrenceOrdinal(0),
        Utc::now() - Duration::seconds(1),
    )
    .expect("sample occurrence planning should pass generated authority");
    let occurrence = occurrence_write.occurrence().clone();
    store.commit_occurrence_write(occurrence_write).await?;
    let claim_token = Uuid::now_v7();
    let (occurrence, _) = store
        .transition_occurrence_if_current(
            &occurrence.occurrence_id,
            occurrence.attempt_count,
            occurrence.claim_token(),
            OccurrenceLifecycleInput::Claim {
                owner_id: "receipt-projection-test".to_string(),
                at_utc: Utc::now(),
                lease_expires_at_utc: Utc::now() + Duration::minutes(5),
                claim_token,
            },
        )
        .await?
        .expect("claim should update current occurrence");
    let (occurrence, _) = store
        .transition_occurrence_if_current(
            &occurrence.occurrence_id,
            occurrence.attempt_count,
            occurrence.claim_token(),
            OccurrenceLifecycleInput::DispatchStarted {
                correlation_id: Some("dispatch-attempt-1".to_string()),
                at_utc: Utc::now(),
            },
        )
        .await?
        .expect("dispatch start should update current occurrence");
    let (occurrence, _) = store
        .transition_occurrence_if_current(
            &occurrence.occurrence_id,
            occurrence.attempt_count,
            occurrence.claim_token(),
            OccurrenceLifecycleInput::AwaitCompletion { at_utc: Utc::now() },
        )
        .await?
        .expect("await completion should update current occurrence");
    Ok(occurrence)
}

async fn assert_append_receipt_updates_occurrence_projection(
    store: Arc<dyn ScheduleStore>,
) -> Result<(), Box<dyn std::error::Error>> {
    let create_mutator = sample_schedule_mutator();
    let schedule = create_mutator.schedule.clone();
    store
        .commit_schedule_write(create_mutator.into_authorized_write())
        .await?;

    let occurrence = sample_in_flight_occurrence(store.as_ref(), &schedule).await?;

    let receipt = occurrence.delivery_receipt_from_authority(None)?;

    store.append_receipt(receipt.clone()).await?;

    let fetched = store
        .get_occurrence(&occurrence.occurrence_id)
        .await?
        .expect("occurrence should exist after receipt append");
    assert_eq!(fetched.last_receipt, Some(receipt.clone()));

    let listed = store
        .list_occurrences(OccurrenceFilter {
            schedule_id: Some(schedule.schedule_id.clone()),
            include_terminal: true,
            ..OccurrenceFilter::default()
        })
        .await?;
    assert_eq!(listed.len(), 1, "expected exactly one stored occurrence");
    assert_eq!(listed[0].last_receipt, Some(receipt.clone()));

    let receipts = store.list_receipts(&occurrence.occurrence_id).await?;
    assert_eq!(receipts, vec![receipt]);
    Ok(())
}

#[tokio::test]
async fn sqlite_append_receipt_updates_occurrence_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Arc::new(SqliteScheduleStore::open(
        dir.path().join("schedule.sqlite3"),
    )?) as Arc<dyn ScheduleStore>;
    assert_append_receipt_updates_occurrence_projection(store).await
}
