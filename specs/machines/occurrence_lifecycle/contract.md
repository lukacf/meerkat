# OccurrenceLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `3`
- Rust owner: `self` / `catalog::dsl::occurrence_lifecycle`

## State
- Phase enum: `Pending | Claimed | Dispatching | AwaitingCompletion | Completed | Skipped | Misfired | Superseded | DeliveryFailed`
- `occurrence_id`: `OccurrenceId`
- `schedule_id`: `ScheduleId`
- `schedule_revision`: `u64`
- `occurrence_ordinal`: `u64`
- `trigger_key`: `String`
- `target_binding_key`: `String`
- `misfire_policy`: `MisfirePolicy`
- `misfire_policy_key`: `String`
- `overlap_policy`: `OverlapPolicy`
- `overlap_policy_key`: `String`
- `missing_target_policy`: `MissingTargetPolicy`
- `missing_target_policy_key`: `String`
- `due_at_utc_ms`: `u64`
- `misfire_deadline_utc_ms`: `u64`
- `claimed_by`: `Option<String>`
- `lease_expires_at_utc_ms`: `Option<u64>`
- `claimed_at_utc_ms`: `Option<u64>`
- `claim_token`: `Option<ClaimToken>`
- `delivery_correlation_id`: `Option<String>`
- `target_materialized_session_id`: `Option<SessionId>`
- `receipt_recorded_at_utc_ms`: `Option<u64>`
- `last_receipt_recorded_at_utc_ms`: `Option<u64>`
- `last_receipt_attempt`: `Option<u64>`
- `last_receipt_stage`: `Option<DeliveryReceiptStage>`
- `last_receipt_failure_class`: `Option<OccurrenceFailureClass>`
- `last_receipt_detail`: `Option<String>`
- `last_receipt_correlation_id`: `Option<String>`
- `last_receipt_materialized_session_id`: `Option<SessionId>`
- `runtime_outcome_key`: `Option<String>`
- `receipt_stage`: `Option<DeliveryReceiptStage>`
- `receipt_failure_class`: `Option<OccurrenceFailureClass>`
- `receipt_detail`: `Option<String>`
- `failure_class`: `Option<OccurrenceFailureClass>`
- `failure_detail`: `Option<String>`
- `dispatched_at_utc_ms`: `Option<u64>`
- `completed_at_utc_ms`: `Option<u64>`
- `attempt_count`: `u64`
- `superseded_by_revision`: `Option<u64>`

## Inputs
- `PlanOccurrence`(occurrence_id: OccurrenceId, schedule_id: ScheduleId, schedule_revision: u64, occurrence_ordinal: u64, trigger_key: String, target_binding_key: String, misfire_policy: MisfirePolicy, misfire_policy_key: String, overlap_policy: OverlapPolicy, overlap_policy_key: String, missing_target_policy: MissingTargetPolicy, missing_target_policy_key: String, target_materialized_session_id: Option<SessionId>, due_at_utc_ms: u64, misfire_deadline_utc_ms: u64)
- `SyncTargetSnapshot`(target_binding_key: String, target_materialized_session_id: Option<SessionId>)
- `RecordReceipt`(correlation_id: Option<String>, detail: Option<String>, materialized_session_id: Option<SessionId>, runtime_outcome_key: Option<String>)
- `ClassifyDue`(now_utc_ms: u64)
- `Claim`(owner_id: String, at_utc_ms: u64, lease_expires_at_utc_ms: u64, claim_token: ClaimToken)
- `DispatchStarted`(correlation_id: Option<String>, at_utc_ms: u64)
- `AwaitCompletion`(at_utc_ms: u64)
- `Complete`(at_utc_ms: u64)
- `ResolveRuntimeCompletion`(outcome: RuntimeCompletionOutcome, detail: Option<String>, at_utc_ms: u64)
- `Skip`(detail: Option<String>, failure_class: Option<OccurrenceFailureClass>, at_utc_ms: u64)
- `Misfire`(detail: Option<String>, failure_class: Option<OccurrenceFailureClass>, at_utc_ms: u64)
- `Supersede`(superseded_by_revision: u64, at_utc_ms: u64)
- `DeliveryFailed`(failure_class: OccurrenceFailureClass, detail: Option<String>, at_utc_ms: u64)
- `LeaseExpired`(at_utc_ms: u64)
- `ReleaseLeaseForPausedSchedule`(at_utc_ms: u64)

## Signals

## Effects
- `Claimed`
- `DispatchStarted`
- `AwaitingCompletion`
- `Completed`
- `Skipped`
- `Misfired`
- `Superseded`
- `OccurrencesSuperseded`(occurrence_id: OccurrenceId, superseding_revision: u64)
- `DueNoAction`
- `DueClaimEligible`
- `DueMisfireRequired`
- `DueLeaseExpired`
- `DeliveryFailed`
- `LeaseExpired`

## Helpers
- `is_live_claim_phase`(phase: OccurrenceLifecycleState) -> `Bool`

## Invariants
- `live_claim_requires_owner`
- `superseded_records_revision`
- `delivery_failed_records_failure_class`
- `misfire_deadline_not_before_due`

## Transitions
### `PlanOccurrenceFromPending`
- From: `Pending`
- On: `PlanOccurrence`(occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, target_materialized_session_id, due_at_utc_ms, misfire_deadline_utc_ms)
- Guards:
  - ``
- To: `Pending`

### `ClassifyDuePendingFuture`
- From: `Pending`
- On: `ClassifyDue`(now_utc_ms)
- Guards:
  - ``
- Emits: `DueNoAction`
- To: `Pending`

### `ClassifyDuePendingMisfire`
- From: `Pending`
- On: `ClassifyDue`(now_utc_ms)
- Guards:
  - ``
- Emits: `DueMisfireRequired`
- To: `Pending`

### `ClassifyDuePendingClaimEligible`
- From: `Pending`
- On: `ClassifyDue`(now_utc_ms)
- Guards:
  - ``
- Emits: `DueClaimEligible`
- To: `Pending`

### `ClassifyDueClaimedLeaseExpired`
- From: `Claimed`
- On: `ClassifyDue`(now_utc_ms)
- Guards:
  - ``
- Emits: `DueLeaseExpired`
- To: `Claimed`

### `ClassifyDueDispatchingLeaseExpired`
- From: `Dispatching`
- On: `ClassifyDue`(now_utc_ms)
- Guards:
  - ``
- Emits: `DueLeaseExpired`
- To: `Dispatching`

### `ClassifyDueAwaitingCompletionLeaseExpired`
- From: `AwaitingCompletion`
- On: `ClassifyDue`(now_utc_ms)
- Guards:
  - ``
- Emits: `DueLeaseExpired`
- To: `AwaitingCompletion`

### `ClassifyDueClaimedLeaseCurrent`
- From: `Claimed`
- On: `ClassifyDue`(now_utc_ms)
- Guards:
  - ``
- Emits: `DueNoAction`
- To: `Claimed`

### `ClassifyDueDispatchingLeaseCurrent`
- From: `Dispatching`
- On: `ClassifyDue`(now_utc_ms)
- Guards:
  - ``
- Emits: `DueNoAction`
- To: `Dispatching`

### `ClassifyDueAwaitingCompletionLeaseCurrent`
- From: `AwaitingCompletion`
- On: `ClassifyDue`(now_utc_ms)
- Guards:
  - ``
- Emits: `DueNoAction`
- To: `AwaitingCompletion`

### `ClassifyDueCompletedNoAction`
- From: `Completed`
- On: `ClassifyDue`(now_utc_ms)
- Emits: `DueNoAction`
- To: `Completed`

### `ClassifyDueSkippedNoAction`
- From: `Skipped`
- On: `ClassifyDue`(now_utc_ms)
- Emits: `DueNoAction`
- To: `Skipped`

### `ClassifyDueMisfiredNoAction`
- From: `Misfired`
- On: `ClassifyDue`(now_utc_ms)
- Emits: `DueNoAction`
- To: `Misfired`

### `ClassifyDueSupersededNoAction`
- From: `Superseded`
- On: `ClassifyDue`(now_utc_ms)
- Emits: `DueNoAction`
- To: `Superseded`

### `ClassifyDueDeliveryFailedNoAction`
- From: `DeliveryFailed`
- On: `ClassifyDue`(now_utc_ms)
- Emits: `DueNoAction`
- To: `DeliveryFailed`

### `SyncTargetSnapshotPending`
- From: `Pending`
- On: `SyncTargetSnapshot`(target_binding_key, target_materialized_session_id)
- To: `Pending`

### `SyncTargetSnapshotClaimed`
- From: `Claimed`
- On: `SyncTargetSnapshot`(target_binding_key, target_materialized_session_id)
- To: `Claimed`

### `RecordReceiptPending`
- From: `Pending`
- On: `RecordReceipt`(correlation_id, detail, materialized_session_id, runtime_outcome_key)
- Guards:
  - ``
- To: `Pending`

### `RecordReceiptClaimed`
- From: `Claimed`
- On: `RecordReceipt`(correlation_id, detail, materialized_session_id, runtime_outcome_key)
- Guards:
  - ``
- To: `Claimed`

### `RecordReceiptDispatching`
- From: `Dispatching`
- On: `RecordReceipt`(correlation_id, detail, materialized_session_id, runtime_outcome_key)
- Guards:
  - ``
- To: `Dispatching`

### `RecordReceiptAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `RecordReceipt`(correlation_id, detail, materialized_session_id, runtime_outcome_key)
- Guards:
  - ``
- To: `AwaitingCompletion`

### `RecordReceiptCompleted`
- From: `Completed`
- On: `RecordReceipt`(correlation_id, detail, materialized_session_id, runtime_outcome_key)
- Guards:
  - ``
- To: `Completed`

### `RecordReceiptSkipped`
- From: `Skipped`
- On: `RecordReceipt`(correlation_id, detail, materialized_session_id, runtime_outcome_key)
- Guards:
  - ``
- To: `Skipped`

### `RecordReceiptMisfired`
- From: `Misfired`
- On: `RecordReceipt`(correlation_id, detail, materialized_session_id, runtime_outcome_key)
- Guards:
  - ``
- To: `Misfired`

### `RecordReceiptSuperseded`
- From: `Superseded`
- On: `RecordReceipt`(correlation_id, detail, materialized_session_id, runtime_outcome_key)
- Guards:
  - ``
- To: `Superseded`

### `RecordReceiptDeliveryFailed`
- From: `DeliveryFailed`
- On: `RecordReceipt`(correlation_id, detail, materialized_session_id, runtime_outcome_key)
- Guards:
  - ``
- To: `DeliveryFailed`

### `ClaimPending`
- From: `Pending`
- On: `Claim`(owner_id, at_utc_ms, lease_expires_at_utc_ms, claim_token)
- Guards:
  - ``
- Emits: `Claimed`
- To: `Claimed`

### `DispatchStartedFromClaimed`
- From: `Claimed`
- On: `DispatchStarted`(correlation_id, at_utc_ms)
- Emits: `DispatchStarted`
- To: `Dispatching`

### `AwaitCompletionFromDispatching`
- From: `Dispatching`
- On: `AwaitCompletion`(at_utc_ms)
- Emits: `AwaitingCompletion`
- To: `AwaitingCompletion`

### `CompleteFromDispatchingOrAwaiting`
- From: `Dispatching`, `AwaitingCompletion`
- On: `Complete`(at_utc_ms)
- Emits: `Completed`
- To: `Completed`

### `RuntimeCompletionCompleted`
- From: `Dispatching`, `AwaitingCompletion`
- On: `ResolveRuntimeCompletion`(outcome, detail, at_utc_ms)
- Guards:
  - `runtime_outcome_completed`
- Emits: `Completed`
- To: `Completed`

### `RuntimeCompletionRuntimeRejected`
- From: `Dispatching`, `AwaitingCompletion`
- On: `ResolveRuntimeCompletion`(outcome, detail, at_utc_ms)
- Guards:
  - `runtime_outcome_rejected`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `RuntimeCompletionTransportError`
- From: `Dispatching`, `AwaitingCompletion`
- On: `ResolveRuntimeCompletion`(outcome, detail, at_utc_ms)
- Guards:
  - `runtime_outcome_transport_error`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `RuntimeCompletionInternalError`
- From: `Dispatching`, `AwaitingCompletion`
- On: `ResolveRuntimeCompletion`(outcome, detail, at_utc_ms)
- Guards:
  - `runtime_outcome_internal_error`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `SkipFromPendingOrLive`
- From: `Pending`, `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `Skip`(detail, failure_class, at_utc_ms)
- Emits: `Skipped`
- To: `Skipped`

### `MisfireFromPendingOrLive`
- From: `Pending`, `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `Misfire`(detail, failure_class, at_utc_ms)
- Emits: `Misfired`
- To: `Misfired`

### `SupersedePendingOrLive`
- From: `Pending`, `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `Supersede`(superseded_by_revision, at_utc_ms)
- Emits: `Superseded`, `OccurrencesSuperseded`
- To: `Superseded`

### `DeliveryFailedFromClaimedOrLive`
- From: `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `DeliveryFailed`(failure_class, detail, at_utc_ms)
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `LeaseExpiredFromClaimed`
- From: `Claimed`
- On: `LeaseExpired`(at_utc_ms)
- Emits: `LeaseExpired`
- To: `Pending`

### `LeaseExpiredFromDispatching`
- From: `Dispatching`
- On: `LeaseExpired`(at_utc_ms)
- Emits: `LeaseExpired`
- To: `Pending`

### `LeaseExpiredFromAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `LeaseExpired`(at_utc_ms)
- Emits: `LeaseExpired`
- To: `Pending`

### `ReleaseLeaseForPausedScheduleFromClaimed`
- From: `Claimed`
- On: `ReleaseLeaseForPausedSchedule`(at_utc_ms)
- Emits: `LeaseExpired`
- To: `Pending`

### `ReleaseLeaseForPausedScheduleFromDispatching`
- From: `Dispatching`
- On: `ReleaseLeaseForPausedSchedule`(at_utc_ms)
- Emits: `LeaseExpired`
- To: `Pending`

### `ReleaseLeaseForPausedScheduleFromAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ReleaseLeaseForPausedSchedule`(at_utc_ms)
- Emits: `LeaseExpired`
- To: `Pending`

## Coverage
### Code Anchors
- `meerkat-schedule/src/lifecycle.rs` — Occurrence::planned_from_schedule and Occurrence::apply domain-facing lifecycle transition seam over plan occurrence from pending, sync target snapshot from pending or claimed materialized bindings, record receipt from pending, claimed, dispatching, awaiting completion, completed, skipped, misfired, superseded, or delivery failed result projection, classify due no action, due claim eligible, due misfire required, due lease expired, claim, claimed, dispatch, await completion, complete, resolve runtime completion outcome, completed, skip, skipped, misfire, misfired, supersede, superseded, delivery failure, lease expiry, live owner, revision, and failure classification

### Scenarios
- `occurrence_start_complete_fail` — occurrence transitions through pending, running, and terminal lifecycle states
- `occurrence_claim_dispatch_completion` — plan occurrence from pending, sync target snapshot from pending or claimed materialized bindings, record receipt from pending, claimed, dispatching, awaiting completion, completed, skipped, misfired, superseded, or delivery failed result projection, claim pending occurrence, dispatch started from claimed, await completion, complete from dispatching or awaiting, resolve runtime completion outcome, and record claimed/dispatch/awaiting/completed effects
- `occurrence_terminal_classification` — skip/skipped, misfire/misfired, supersede/superseded, delivery failed, occurrences superseded, records revision and explicit failure class for terminal occurrence outcomes
- `occurrence_lease_recovery` — classify due no action, due claim eligible, due misfire required, due lease expired, and lease expired from claimed, dispatching, or awaiting completion returns live claimed work to owner-aware recovery
