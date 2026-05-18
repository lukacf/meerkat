#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    CreateScheduleRequest, DeliveryReceipt, DeliveryReceiptStage, MisfirePolicy,
    MissingTargetPolicy, Occurrence, OccurrenceFilter, OccurrenceLifecycleInput, OccurrenceOrdinal,
    OverlapPolicy, Schedule, ScheduleStore, ScheduledSessionAction, SessionTargetBinding,
    TargetBinding, TriggerSpec,
};
use meerkat_store::SqliteScheduleStore;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

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
    })
    .expect("sample schedule creation should pass generated authority")
}

fn sample_in_flight_occurrence(schedule: &Schedule) -> Occurrence {
    let occurrence = Occurrence::planned_from_schedule(
        schedule,
        OccurrenceOrdinal(0),
        Utc::now() - Duration::seconds(1),
    )
    .expect("sample occurrence planning should pass generated authority");
    let claim_token = Uuid::now_v7();
    let occurrence = occurrence
        .apply(OccurrenceLifecycleInput::Claim {
            owner_id: "receipt-projection-test".to_string(),
            at_utc: Utc::now(),
            lease_expires_at_utc: Utc::now() + Duration::minutes(5),
            claim_token,
        })
        .expect("claim should pass generated authority")
        .into_occurrence();
    let occurrence = occurrence
        .apply(OccurrenceLifecycleInput::DispatchStarted {
            correlation_id: Some("dispatch-attempt-1".to_string()),
            at_utc: Utc::now(),
        })
        .expect("dispatch start should pass generated authority")
        .into_occurrence();
    occurrence
        .apply(OccurrenceLifecycleInput::AwaitCompletion { at_utc: Utc::now() })
        .expect("await completion should pass generated authority")
        .into_occurrence()
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
async fn sqlite_append_receipt_updates_occurrence_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = Arc::new(SqliteScheduleStore::open(
        dir.path().join("schedule.sqlite3"),
    )?) as Arc<dyn ScheduleStore>;
    assert_append_receipt_updates_occurrence_projection(store).await
}
