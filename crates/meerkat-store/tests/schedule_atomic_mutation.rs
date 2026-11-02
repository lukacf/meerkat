#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    ClaimDueRequest, CreateScheduleRequest, MisfirePolicy, MissingTargetPolicy, Occurrence,
    OccurrenceFilter, OccurrenceOrdinal, OccurrencePhase, OverlapPolicy, Schedule,
    ScheduleLifecycleInput, ScheduleLifecycleMutator, SchedulePhase, ScheduleService,
    ScheduleStore, ScheduledSessionAction, SessionTargetBinding, TargetBinding, TriggerSpec,
    UpdateScheduleRequest,
};
use meerkat_store::SqliteScheduleStore;
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

fn sample_schedule_request() -> CreateScheduleRequest {
    CreateScheduleRequest {
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
    }
}

fn sample_schedule_mutator() -> ScheduleLifecycleMutator {
    Schedule::apply(
        None,
        ScheduleLifecycleInput::Create(sample_schedule_request()),
    )
    .expect("sample schedule creation should pass generated authority")
}

async fn assert_atomic_schedule_mutation_supersedes_old_pending(
    store: Arc<dyn ScheduleStore>,
) -> Result<(), Box<dyn std::error::Error>> {
    let create_mutator = sample_schedule_mutator();
    let original = create_mutator.schedule().clone();
    let _commit = store
        .commit_schedule_write(create_mutator.into_authorized_write())
        .await?;

    let old_pending_write = Occurrence::planned_write_from_schedule(
        &original,
        original.next_occurrence_ordinal,
        Utc::now() + Duration::minutes(6),
    )
    .expect("old occurrence planning should pass generated authority");
    let old_pending = old_pending_write.occurrence().clone();
    let planned_commit = store.commit_occurrence_write(old_pending_write).await?;
    planned_commit
        .prior()
        .check_current(None)
        .expect("planned occurrence commit must retain its absent-row prior CAS");
    assert_eq!(planned_commit.successor(), &old_pending);
    assert!(planned_commit.effects().is_empty());
    assert!(planned_commit.supersession_acks().is_empty());

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
    let mut update_mutator = update_mutator;
    let planning_mutator = Schedule::apply(
        Some(update_mutator.schedule().clone()),
        ScheduleLifecycleInput::RecordPlanningWindow {
            planning_cursor_utc: Utc::now() + Duration::minutes(6),
            next_occurrence_ordinal: original.next_occurrence_ordinal.next(),
        },
    )
    .expect("planning window should pass generated authority");
    update_mutator
        .absorb_followup(planning_mutator)
        .expect("planning window should chain from generated schedule authority");
    let updated = update_mutator.schedule().clone();

    let replacement_write = Occurrence::planned_write_from_schedule(
        &updated,
        updated.next_occurrence_ordinal,
        Utc::now() + Duration::minutes(10),
    )
    .expect("replacement occurrence planning should pass generated authority");
    let replacement = replacement_write.occurrence().clone();

    let committed = store
        .commit_schedule_mutation(
            update_mutator.into_authorized_write(),
            vec![replacement_write],
        )
        .await?;
    let (schedule_commit, occurrence_commits) = committed.into_parts();
    let committed_schedule = schedule_commit.successor();
    let supersession_commit = occurrence_commits
        .iter()
        .find(|commit| commit.successor().occurrence_id == old_pending.occurrence_id)
        .expect("atomic mutation should return the swept occurrence commit");
    assert_eq!(
        supersession_commit.successor().phase,
        OccurrencePhase::Superseded
    );
    assert_eq!(
        supersession_commit.successor().superseded_by_revision,
        Some(updated.revision)
    );
    assert_eq!(supersession_commit.supersession_acks().len(), 1);
    assert_eq!(
        supersession_commit.supersession_acks()[0].occurrence_id(),
        &old_pending.occurrence_id
    );

    let stored = store
        .get_schedule(&updated.schedule_id)
        .await?
        .expect("updated schedule should exist");
    assert_eq!(stored.revision, updated.revision);
    assert!(
        committed_schedule
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

/// The claim scan is bounded in SQL by the phase COLUMN — a write-coherent
/// projection of the canonical schedule JSON — so terminal history never
/// pays per-tick deserialization (ask 19). Within everything the prefilter
/// admits, the CANONICAL JSON stays the deciding authority: a schedule whose
/// canonical state is a tombstone claims nothing even when its projection
/// column still reads 'active'.
#[tokio::test]
async fn sqlite_claim_due_canonical_phase_decides_within_the_column_prefilter()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path)?;

    let create_mutator = sample_schedule_mutator();
    let schedule = create_mutator.schedule().clone();
    let _commit = store
        .commit_schedule_write(create_mutator.into_authorized_write())
        .await?;
    let occurrence_write = Occurrence::planned_write_from_schedule(
        &schedule,
        OccurrenceOrdinal(0),
        Utc::now() - Duration::seconds(1),
    )
    .expect("due occurrence planning should pass generated authority");
    let planned_commit = store.commit_occurrence_write(occurrence_write).await?;
    assert_eq!(planned_commit.successor().phase, OccurrencePhase::Pending);
    assert!(planned_commit.effects().is_empty());
    assert!(planned_commit.supersession_acks().is_empty());

    // Canonically delete the schedule, then force the projection COLUMN back
    // to 'active' out of band: the prefilter admits the row, and the
    // canonical tombstone must still refuse the claim.
    let deleted = meerkat_schedule::Schedule::apply(
        Some(schedule.clone()),
        ScheduleLifecycleInput::Delete { at_utc: Utc::now() },
    )
    .expect("delete should pass generated authority");
    let deleted_commit = store
        .commit_schedule_mutation(deleted.into_authorized_write(), Vec::new())
        .await?;
    let (schedule_commit, occurrence_commits) = deleted_commit.into_parts();
    assert_eq!(schedule_commit.successor().phase, SchedulePhase::Deleted);
    assert_eq!(occurrence_commits.len(), 1);
    assert_eq!(
        occurrence_commits[0].successor().phase,
        OccurrencePhase::Superseded
    );
    let conn = Connection::open(&path)?;
    conn.execute(
        "UPDATE schedule_schedules SET phase = 'active' WHERE schedule_id = ?1",
        [schedule.schedule_id.to_string()],
    )?;
    // Re-arm the occurrence row the tombstone superseded so the prefilter
    // would admit it if the canonical phase did not refuse first.
    conn.execute(
        "UPDATE schedule_occurrences SET phase = 'pending' WHERE schedule_id = ?1",
        [schedule.schedule_id.to_string()],
    )?;
    drop(conn);

    let executor_lease = match store
        .acquire_executor_lease(meerkat_schedule::AcquireScheduleExecutorLeaseRequest {
            owner_id: "sqlite-canonical-phase-executor".into(),
            lease_duration: Duration::minutes(5),
        })
        .await?
    {
        meerkat_schedule::AcquireScheduleExecutorLeaseOutcome::Acquired(lease) => lease,
        meerkat_schedule::AcquireScheduleExecutorLeaseOutcome::Busy { .. } => unreachable!(),
    };
    let claimed = store
        .claim_due_occurrences(
            &executor_lease,
            ClaimDueRequest {
                owner_id: "sqlite-canonical-phase-test".to_string(),
                limit: 1,
                lease_duration: Duration::minutes(5),
            },
        )
        .await?;

    assert!(
        claimed.transitions.is_empty(),
        "a canonical tombstone must not claim even when its projection column reads active"
    );
    Ok(())
}

/// Every schedule write keeps the phase COLUMN coherent with the canonical
/// JSON — the invariant that makes bounding the claim scan on the column a
/// legitimate projection read.
#[tokio::test]
async fn sqlite_schedule_writes_keep_phase_column_coherent()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("schedule.sqlite3");
    let store = SqliteScheduleStore::open(&path)?;

    let create_mutator = sample_schedule_mutator();
    let schedule = create_mutator.schedule().clone();
    let _commit = store
        .commit_schedule_write(create_mutator.into_authorized_write())
        .await?;
    let column_phase = |path: &std::path::Path| -> Result<String, Box<dyn std::error::Error>> {
        let conn = Connection::open(path)?;
        Ok(conn.query_row(
            "SELECT phase FROM schedule_schedules WHERE schedule_id = ?1",
            [schedule.schedule_id.to_string()],
            |row| row.get(0),
        )?)
    };
    assert_eq!(column_phase(&path)?, "active");

    let deleted = meerkat_schedule::Schedule::apply(
        Some(schedule.clone()),
        ScheduleLifecycleInput::Delete { at_utc: Utc::now() },
    )
    .expect("delete should pass generated authority");
    let deleted_commit = store
        .commit_schedule_mutation(deleted.into_authorized_write(), Vec::new())
        .await?;
    let (schedule_commit, occurrence_commits) = deleted_commit.into_parts();
    assert_eq!(schedule_commit.successor().phase, SchedulePhase::Deleted);
    assert!(occurrence_commits.is_empty());
    assert_eq!(column_phase(&path)?, "deleted");
    Ok(())
}

/// A schedule row written before the bound on `superseded_ack_ids` (0.9.8 and
/// earlier) carries the acknowledgements of every earlier supersession in
/// both copies of its JSON. The store loads it unchanged, with no migration,
/// and the next revise through the service drops them durably, keeping only
/// its own supersession's acknowledgements.
#[tokio::test]
async fn sqlite_stored_large_ack_set_loads_and_the_next_revise_drops_it()
-> Result<(), Box<dyn std::error::Error>> {
    const STORED_ACKS: usize = 256;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("schedule.sqlite3");
    let store = Arc::new(SqliteScheduleStore::open(&path)?) as Arc<dyn ScheduleStore>;
    let service = ScheduleService::new(store.clone());
    let created = service.create(sample_schedule_request()).await?;
    let revised = service
        .update(
            &created.schedule_id,
            UpdateScheduleRequest {
                expected_revision: Some(created.revision),
                trigger: Some(TriggerSpec::Once {
                    due_at_utc: Utc::now() + Duration::minutes(20),
                }),
                ..UpdateScheduleRequest::default()
            },
        )
        .await?;

    // The row as a 0.9.8 store left it after many edits: the acknowledgements
    // of this revise and of every earlier one, in both copies.
    let mut stored_ids = revised
        .superseded_ack_ids
        .iter()
        .map(ToString::to_string)
        .collect::<BTreeSet<_>>();
    while stored_ids.len() < STORED_ACKS {
        stored_ids.insert(uuid::Uuid::now_v7().to_string());
    }
    let schedule_key = revised.schedule_id.to_string();
    let conn = Connection::open(&path)?;
    let bytes: Vec<u8> = conn.query_row(
        "SELECT schedule_json FROM schedule_schedules WHERE schedule_id = ?1",
        [schedule_key.as_str()],
        |row| row.get(0),
    )?;
    let mut row: serde_json::Value = serde_json::from_slice(&bytes)?;
    row["superseded_ack_ids"] = serde_json::json!(stored_ids);
    row["machine_state"]["superseded_ack_ids"] = serde_json::json!(stored_ids);
    let changed = conn.execute(
        "UPDATE schedule_schedules SET schedule_json = ?1 WHERE schedule_id = ?2",
        rusqlite::params![serde_json::to_vec(&row)?, schedule_key],
    )?;
    assert_eq!(changed, 1);
    drop(conn);

    let loaded = service.get(&revised.schedule_id).await?;
    assert_eq!(loaded.revision, revised.revision);
    assert_eq!(
        loaded
            .superseded_ack_ids
            .iter()
            .map(ToString::to_string)
            .collect::<BTreeSet<_>>(),
        stored_ids,
        "the stored set loads unchanged"
    );

    let next = service
        .update(
            &revised.schedule_id,
            UpdateScheduleRequest {
                expected_revision: Some(loaded.revision),
                trigger: Some(TriggerSpec::Once {
                    due_at_utc: Utc::now() + Duration::minutes(30),
                }),
                ..UpdateScheduleRequest::default()
            },
        )
        .await?;
    let latest = store
        .list_occurrences(OccurrenceFilter {
            schedule_id: Some(next.schedule_id.clone()),
            include_terminal: true,
            ..OccurrenceFilter::default()
        })
        .await?
        .into_iter()
        .filter(|occurrence| occurrence.superseded_by_revision == Some(next.revision))
        .map(|occurrence| occurrence.occurrence_id)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        latest.len(),
        1,
        "the next revise supersedes the pending occurrence of the previous revision"
    );
    assert_eq!(
        next.superseded_ack_ids, latest,
        "the next revise drops the stored acknowledgements and keeps its own"
    );
    let reloaded = store
        .get_schedule(&next.schedule_id)
        .await?
        .expect("revised schedule row");
    assert_eq!(reloaded.superseded_ack_ids, next.superseded_ack_ids);
    Ok(())
}
