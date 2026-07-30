//! Exact lease-witness evidence for the SQLite schedule store.
//!
//! Two independently opened store handles model separate hosts. Renewal is
//! authorized only by the durable `{ occurrence, attempt, claim_token }`
//! witness and store time; there is no process-local waiter registry that can
//! suppress another host's reclaim decision.

#![cfg(feature = "sqlite")]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::{Duration, Utc};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    ClaimDueRequest, CreateScheduleRequest, DeliveryReceiptStage, MisfirePolicy,
    MissingTargetPolicy, Occurrence, OccurrenceLifecycleInput, OccurrenceOrdinal, OccurrencePhase,
    OverlapPolicy, RenewOccurrenceLeaseOutcome, RenewOccurrenceLeaseRequest, Schedule,
    ScheduleLifecycleInput, ScheduleStore, ScheduledSessionAction, SessionTargetBinding,
    TargetBinding, TriggerSpec,
};
use meerkat_store::SqliteScheduleStore;
use std::collections::BTreeMap;

fn sample_schedule_request(name: &str) -> CreateScheduleRequest {
    CreateScheduleRequest {
        name: Some(name.to_string()),
        description: None,
        trigger: TriggerSpec::Once {
            due_at_utc: Utc::now() - Duration::seconds(1),
        },
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

async fn commit_occurrence_due_at(
    store: &SqliteScheduleStore,
    schedule: &Schedule,
    due_at_utc: chrono::DateTime<Utc>,
) -> Occurrence {
    let write = Occurrence::planned_write_from_schedule(schedule, OccurrenceOrdinal(0), due_at_utc)
        .expect("occurrence planning should pass generated authority");
    let occurrence = write.occurrence().clone();
    store
        .commit_occurrence_write(write)
        .await
        .expect("commit occurrence");
    occurrence
}

fn claim_request(owner_id: &str, lease_duration: Duration) -> ClaimDueRequest {
    ClaimDueRequest {
        owner_id: owner_id.to_string(),
        limit: 8,
        lease_duration,
    }
}

async fn claim_and_dispatch(
    store: &SqliteScheduleStore,
    owner_id: &str,
    lease: Duration,
) -> Occurrence {
    let claimed = store
        .claim_due_occurrences(claim_request(owner_id, lease))
        .await
        .expect("claim due occurrences");
    let occurrence = claimed
        .claimed
        .into_iter()
        .next()
        .expect("a due occurrence should be claimed");
    let dispatch_mutator = occurrence
        .apply(OccurrenceLifecycleInput::DispatchStarted {
            correlation_id: Some(format!("{owner_id}-dispatch")),
            at_utc: claimed.store_now_utc,
        })
        .expect("dispatch should pass generated authority");
    let dispatching = dispatch_mutator.occurrence.clone();
    store
        .commit_occurrence_write(dispatch_mutator.into_authorized_write())
        .await
        .expect("commit dispatch");
    let await_mutator = dispatching
        .apply(OccurrenceLifecycleInput::AwaitCompletion {
            at_utc: claimed.store_now_utc,
        })
        .expect("await should pass generated authority");
    let awaiting = await_mutator.occurrence.clone();
    store
        .commit_occurrence_write(await_mutator.into_authorized_write())
        .await
        .expect("commit await");
    awaiting
}

#[tokio::test]
async fn sqlite_lease_renewal_is_multi_host_and_exact_claim_authoritative() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("schedules.sqlite3");
    let host_a = SqliteScheduleStore::open(&path).expect("open host A store");
    let host_b = SqliteScheduleStore::open(&path).expect("open host B store");

    let schedule = commit_schedule(&host_a, "multi-host-renewal").await;
    let occurrence =
        commit_occurrence_due_at(&host_a, &schedule, Utc::now() - Duration::seconds(1)).await;
    let awaiting = claim_and_dispatch(&host_a, "host-a", Duration::milliseconds(200)).await;
    assert_eq!(awaiting.occurrence_id, occurrence.occurrence_id);
    let original_expiry = awaiting
        .lease_expires_at_utc
        .expect("claimed occurrence has a lease");
    let original_claim_token = awaiting.claim_token().expect("claim token");

    // A separately opened store handle can renew host A's exact durable
    // claim. This proves renewal is store authority rather than an in-process
    // waiter exemption.
    let renewal_request = RenewOccurrenceLeaseRequest {
        occurrence_id: occurrence.occurrence_id.clone(),
        expected_attempt: awaiting.attempt_count,
        claim_token: original_claim_token,
        expected_owner_id: "host-a".to_string(),
        lease_duration: Duration::milliseconds(500),
    };
    let wrong_owner = host_b
        .renew_occurrence_lease_if_current(RenewOccurrenceLeaseRequest {
            expected_owner_id: "host-b".to_string(),
            ..renewal_request.clone()
        })
        .await
        .expect("wrong-owner evidence is a typed outcome");
    assert!(matches!(
        wrong_owner.outcome,
        RenewOccurrenceLeaseOutcome::StaleClaim
    ));
    let after_wrong_owner = host_a
        .get_occurrence(&occurrence.occurrence_id)
        .await
        .expect("get occurrence after wrong-owner renewal")
        .expect("occurrence exists");
    assert_eq!(
        after_wrong_owner.lease_expires_at_utc,
        Some(original_expiry),
        "wrong-owner renewal must not mutate the durable lease"
    );

    let renewed = host_b
        .renew_occurrence_lease_if_current(renewal_request.clone())
        .await
        .expect("host B renews exact host A claim");
    let renewed = match renewed.outcome {
        RenewOccurrenceLeaseOutcome::Renewed(renewed) => Some(renewed),
        RenewOccurrenceLeaseOutcome::StaleClaim => None,
    };
    assert!(
        renewed.is_some(),
        "the exact durable claim witness must renew across hosts"
    );
    let renewed = renewed.unwrap();
    let renewed_expiry = renewed
        .lease_expires_at_utc
        .expect("renewed occurrence has a lease");
    assert!(renewed_expiry > original_expiry);
    assert_eq!(renewed.attempt_count, 1);

    // Past the original expiry but before the renewed expiry, another host's
    // claim scan must observe the durable extension and leave attempt 1 live.
    tokio::time::sleep(std::time::Duration::from_millis(240)).await;
    let before_renewed_expiry = host_a
        .claim_due_occurrences(claim_request("host-a", Duration::milliseconds(200)))
        .await
        .expect("claim scan before renewed expiry");
    assert!(
        before_renewed_expiry.claimed.is_empty(),
        "durable cross-host renewal must prevent premature reclaim"
    );
    let still_awaiting = host_a
        .get_occurrence(&occurrence.occurrence_id)
        .await
        .expect("get renewed occurrence")
        .expect("renewed occurrence exists");
    assert_eq!(still_awaiting.phase, OccurrencePhase::AwaitingCompletion);
    assert_eq!(still_awaiting.attempt_count, 1);

    // Once the renewed lease truly expires, another host reclaims it. The old
    // attempt's renewal request must then return typed StaleClaim and must not
    // extend or otherwise mutate attempt 2.
    let remaining = renewed_expiry.signed_duration_since(Utc::now());
    if let Ok(remaining) = remaining.to_std() {
        tokio::time::sleep(remaining + std::time::Duration::from_millis(25)).await;
    }
    let reclaimed = host_b
        .claim_due_occurrences(claim_request("host-b", Duration::seconds(2)))
        .await
        .expect("claim after renewed lease expiry");
    assert_eq!(reclaimed.claimed.len(), 1);
    assert_eq!(reclaimed.claimed[0].attempt_count, 2);
    let attempt_two_expiry = reclaimed.claimed[0].lease_expires_at_utc;

    let stale = host_a
        .renew_occurrence_lease_if_current(renewal_request)
        .await
        .expect("stale renewal is a typed outcome, not a store error");
    assert!(matches!(
        stale.outcome,
        RenewOccurrenceLeaseOutcome::StaleClaim
    ));
    let current = host_a
        .get_occurrence(&occurrence.occurrence_id)
        .await
        .expect("get reclaimed occurrence")
        .expect("reclaimed occurrence exists");
    assert_eq!(current.attempt_count, 2);
    assert_eq!(
        current.lease_expires_at_utc, attempt_two_expiry,
        "stale attempt 1 renewal must not mutate attempt 2"
    );

    let receipts = host_a
        .list_receipts(&occurrence.occurrence_id)
        .await
        .expect("list receipts");
    assert!(
        receipts
            .iter()
            .any(|receipt| receipt.stage == DeliveryReceiptStage::LeaseExpired),
        "real expiry and reclaim must remain durably observable"
    );
}
