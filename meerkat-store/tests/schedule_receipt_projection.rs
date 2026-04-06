#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    CreateScheduleRequest, DeliveryReceipt, DeliveryReceiptStage, MisfirePolicy,
    MissingTargetPolicy, Occurrence, OccurrenceFilter, OccurrenceOrdinal, OccurrencePhase,
    OverlapPolicy, Schedule, ScheduleStore, ScheduledSessionAction, SessionTargetBinding,
    TargetBinding, TriggerSpec,
};
use meerkat_store::RedbScheduleStore;
#[cfg(feature = "sqlite")]
use meerkat_store::SqliteScheduleStore;
use std::collections::BTreeMap;
use std::sync::Arc;

fn sample_schedule() -> Schedule {
    Schedule::new(CreateScheduleRequest {
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
                skill_references: Vec::new(),
                additional_instructions: Vec::new(),
            },
        }),
        misfire_policy: MisfirePolicy::Skip,
        overlap_policy: OverlapPolicy::SkipIfRunning,
        missing_target_policy: MissingTargetPolicy::MarkMisfired,
        labels: BTreeMap::new(),
        planning_horizon_days: Some(1),
        planning_horizon_occurrences: Some(1),
    })
}

fn sample_in_flight_occurrence(schedule: &Schedule) -> Occurrence {
    let mut occurrence = Occurrence::planned_from_schedule(
        schedule,
        OccurrenceOrdinal(0),
        Utc::now() + Duration::minutes(1),
    );
    occurrence.phase = OccurrencePhase::AwaitingCompletion;
    occurrence.attempt_count = 1;
    occurrence.delivery_correlation_id = Some("dispatch-attempt-1".to_string());
    occurrence
}

async fn assert_append_receipt_updates_occurrence_projection(
    store: Arc<dyn ScheduleStore>,
) -> Result<(), Box<dyn std::error::Error>> {
    let schedule = sample_schedule();
    store.put_schedule(schedule.clone()).await?;

    let occurrence = sample_in_flight_occurrence(&schedule);
    store.put_occurrence(occurrence.clone()).await?;

    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchStarted,
    );
    receipt.correlation_id = Some("dispatch-attempt-1".to_string());

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
async fn redb_append_receipt_updates_occurrence_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Arc::new(RedbScheduleStore::open(dir.path().join("schedule.redb"))?)
        as Arc<dyn ScheduleStore>;
    assert_append_receipt_updates_occurrence_projection(store).await
}

#[cfg(feature = "sqlite")]
#[tokio::test]
async fn sqlite_append_receipt_updates_occurrence_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Arc::new(SqliteScheduleStore::open(
        dir.path().join("schedule.sqlite3"),
    )?) as Arc<dyn ScheduleStore>;
    assert_append_receipt_updates_occurrence_projection(store).await
}
