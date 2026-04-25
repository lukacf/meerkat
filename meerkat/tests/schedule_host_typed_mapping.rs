//! Schedule-host typed-mapping surface tests (C-T §6 #11).
//!
//! The pre-wave-a schedule-host suite covered "dispatch-from-admission +
//! two scheduled-completion-future mappings". In wave-c the canonical
//! translation primitives live at the facade surface level —
//! `meerkat::surface::{immediate_delivery_failure,
//! schedule_attempt_idempotency_key, immediate_completed_dispatch,
//! async_completion_dispatch}` — and every binding surface (CLI, REST,
//! RPC) routes through them. This test pins the typed shape of those
//! functions so a refactor that drops a field, swaps a receipt stage,
//! or rewrites the idempotency key silently is caught by a shared
//! surface test rather than three near-identical surface tests.
//!
//! See `docs/wave-c-prep/test-coverage-audit.md` §6 #11.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use meerkat::surface::{
    async_completion_dispatch, immediate_completed_dispatch, immediate_delivery_failure,
    schedule_attempt_idempotency_key,
};
use meerkat_core::{ContentInput, SessionId};
use meerkat_schedule::{
    CreateScheduleRequest, DeliveryReceiptStage, DeliveryTerminal, IntervalTriggerSpec,
    MisfirePolicy, MissingTargetPolicy, Occurrence, OccurrenceFailureClass, OccurrenceOrdinal,
    OccurrencePhase, OverlapPolicy, Schedule, ScheduledSessionAction, SessionTargetBinding,
    TargetBinding, TriggerSpec,
};

fn sample_occurrence(attempt_count: u32) -> Occurrence {
    let schedule = Schedule::new(CreateScheduleRequest {
        name: Some("c-t-mapping-test".to_string()),
        description: None,
        trigger: TriggerSpec::Interval(IntervalTriggerSpec {
            start_at_utc: chrono::Utc::now(),
            every_seconds: 60,
            end_at_utc: None,
        }),
        target: TargetBinding::session(SessionTargetBinding::ExactSession {
            session_id: SessionId::new(),
            action: ScheduledSessionAction::Prompt {
                prompt: ContentInput::Text("hi".into()),
                system_prompt: None,
                render_metadata: None,
                skill_references: Vec::new(),
                additional_instructions: Vec::new(),
            },
        }),
        misfire_policy: MisfirePolicy::Skip,
        overlap_policy: OverlapPolicy::SkipIfRunning,
        missing_target_policy: MissingTargetPolicy::Skip,
        labels: BTreeMap::new(),
        planning_horizon_days: None,
        planning_horizon_occurrences: None,
    });

    let mut occ =
        Occurrence::planned_from_schedule(&schedule, OccurrenceOrdinal(0), chrono::Utc::now());
    occ.attempt_count = attempt_count;
    occ
}

#[test]
fn idempotency_key_encodes_schedule_occurrence_and_attempt() {
    let occ = sample_occurrence(3);
    let key = schedule_attempt_idempotency_key(&occ);

    let expected = format!(
        "schedule:{}:occurrence:{}:attempt:3",
        occ.schedule_id, occ.occurrence_id
    );
    assert_eq!(key, expected, "idempotency key shape must be stable");

    // Changing attempt_count must produce a distinct key — this is what
    // makes the key safe to use as the de-dup fingerprint for retries.
    let occ_other = sample_occurrence(4);
    // Same schedule+occurrence would require identical IDs; instead we
    // assert by rebuilding the key against a new attempt count directly
    // on the same occurrence struct via a parallel fixture with the
    // same schedule_id/occurrence_id is impossible without internals,
    // so we verify the attempt-count segment is what varies.
    let key_other = schedule_attempt_idempotency_key(&occ_other);
    assert!(
        key_other.ends_with(":attempt:4"),
        "attempt segment must match attempt_count; got {key_other:?}"
    );
    assert!(key.ends_with(":attempt:3"));
}

#[tokio::test]
async fn immediate_delivery_failure_carries_typed_failure_class_and_detail() {
    let occ = sample_occurrence(2);
    let session_id = SessionId::new();

    let dispatch = immediate_delivery_failure(
        &occ,
        "target unreachable".to_string(),
        OccurrenceFailureClass::TargetMissing,
        Some("corr-123".to_string()),
        Some(session_id.clone()),
    );

    // Receipt shape: DispatchStarted stage, correlation + materialized
    // session propagated verbatim.
    assert_eq!(
        dispatch.receipt.stage,
        DeliveryReceiptStage::DispatchStarted,
        "failure dispatch must start at DispatchStarted — it never \
         reaches Accepted",
    );
    assert_eq!(dispatch.receipt.correlation_id.as_deref(), Some("corr-123"));
    assert_eq!(
        dispatch.receipt.materialized_session_id.as_ref(),
        Some(&session_id),
    );
    assert_eq!(dispatch.correlation_id.as_deref(), Some("corr-123"));
    assert_eq!(dispatch.materialized_session_id.as_ref(), Some(&session_id));

    // Completion future resolves immediately to DeliveryFailed carrying
    // the typed failure class + detail.
    let terminal: DeliveryTerminal = dispatch.completion.await.unwrap();
    assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);
    assert_eq!(terminal.detail.as_deref(), Some("target unreachable"));
    assert_eq!(
        terminal.failure_class,
        Some(OccurrenceFailureClass::TargetMissing),
    );
    assert!(
        terminal.runtime_outcome.is_none(),
        "immediate-failure path has no runtime outcome; got {:?}",
        terminal.runtime_outcome,
    );
    assert_eq!(terminal.materialized_session_id, Some(session_id));
}

#[tokio::test]
async fn immediate_completed_dispatch_maps_to_accepted_stage_and_completes() {
    let occ = sample_occurrence(1);
    let dispatch = immediate_completed_dispatch(&occ, Some("corr-immediate".to_string()));

    assert_eq!(
        dispatch.receipt.stage,
        DeliveryReceiptStage::DispatchAccepted,
        "success path starts already-accepted",
    );
    assert_eq!(
        dispatch.receipt.correlation_id.as_deref(),
        Some("corr-immediate"),
    );
    assert!(
        dispatch.materialized_session_id.is_none(),
        "immediate_completed_dispatch does not rematerialize; got {:?}",
        dispatch.materialized_session_id,
    );

    let terminal = dispatch.completion.await.unwrap();
    assert!(
        matches!(terminal.phase, OccurrencePhase::Completed),
        "immediate completion maps to OccurrencePhase::Completed; got {:?}",
        terminal.phase,
    );
    assert!(terminal.failure_class.is_none());
    assert!(terminal.detail.is_none());
}

#[tokio::test]
async fn async_completion_dispatch_threads_correlation_and_custom_future() {
    let occ = sample_occurrence(1);
    let correlation = Some("corr-async".to_string());

    // Build a custom completion future that lands on DeliveryFailed with
    // a runtime-rejected class — this simulates the
    // admission-rejected-after-dispatch path.
    let custom_future = Box::pin(async move {
        Ok(DeliveryTerminal {
            phase: OccurrencePhase::DeliveryFailed,
            receipt: None,
            detail: Some("simulated runtime rejection".to_string()),
            failure_class: Some(OccurrenceFailureClass::RuntimeRejected),
            runtime_outcome: None,
            materialized_session_id: None,
        })
    });

    let dispatch = async_completion_dispatch(&occ, correlation.clone(), custom_future);

    assert_eq!(
        dispatch.receipt.stage,
        DeliveryReceiptStage::DispatchAccepted,
        "async dispatch is already-accepted; completion lands later",
    );
    assert_eq!(dispatch.correlation_id, correlation);

    let terminal = dispatch.completion.await.unwrap();
    assert_eq!(terminal.phase, OccurrencePhase::DeliveryFailed);
    assert_eq!(
        terminal.failure_class,
        Some(OccurrenceFailureClass::RuntimeRejected),
    );
}

#[test]
fn idempotency_key_is_stable_across_calls_for_same_occurrence() {
    let occ = sample_occurrence(1);
    // Calling the function twice with the same occurrence must return
    // byte-identical strings — anything less defeats de-dup by key.
    let a = schedule_attempt_idempotency_key(&occ);
    let b = schedule_attempt_idempotency_key(&occ);
    assert_eq!(a, b);
}
