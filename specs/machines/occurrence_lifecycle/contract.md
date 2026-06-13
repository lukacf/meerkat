# OccurrenceLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `8`
- Rust owner: `self` / `catalog::dsl::occurrence_lifecycle`

## State
- Phase enum: `Pending | Claimed | Dispatching | AwaitingCompletion | Completed | Skipped | Misfired | Superseded | DeliveryFailed`
- `occurrence_id`: `OccurrenceId`
- `schedule_id`: `ScheduleId`
- `schedule_revision`: `u64`
- `occurrence_ordinal`: `u64`
- `trigger_key`: `TriggerKey`
- `target_binding_key`: `TargetBindingId`
- `misfire_policy`: `MisfirePolicy`
- `misfire_policy_key`: `String`
- `overlap_policy`: `OverlapPolicy`
- `overlap_policy_key`: `String`
- `missing_target_policy`: `MissingTargetPolicy`
- `missing_target_policy_key`: `String`
- `due_at_utc_ms`: `u64`
- `misfire_deadline_utc_ms`: `u64`
- `claimed_by`: `Option<ClaimOwner>`
- `lease_expires_at_utc_ms`: `Option<u64>`
- `claimed_at_utc_ms`: `Option<u64>`
- `claim_token`: `Option<ClaimToken>`
- `delivery_correlation_id`: `Option<CorrelationId>`
- `target_materialized_session_id`: `Option<SessionId>`
- `receipt_recorded_at_utc_ms`: `Option<u64>`
- `last_receipt_recorded_at_utc_ms`: `Option<u64>`
- `last_receipt_attempt`: `Option<u64>`
- `last_receipt_stage`: `Option<DeliveryReceiptStage>`
- `last_receipt_failure_class`: `Option<OccurrenceFailureClass>`
- `last_receipt_detail`: `Option<String>`
- `last_receipt_correlation_id`: `Option<CorrelationId>`
- `last_receipt_materialized_session_id`: `Option<SessionId>`
- `runtime_outcome_key`: `Option<RuntimeOutcomeKey>`
- `receipt_stage`: `Option<DeliveryReceiptStage>`
- `receipt_failure_class`: `Option<OccurrenceFailureClass>`
- `receipt_detail`: `Option<String>`
- `failure_class`: `Option<OccurrenceFailureClass>`
- `failure_detail`: `Option<String>`
- `dispatched_at_utc_ms`: `Option<u64>`
- `completed_at_utc_ms`: `Option<u64>`
- `attempt_count`: `u64`
- `superseded_by_revision`: `Option<u64>`
- `late_completion_recorded_at_utc_ms`: `Option<u64>`
- `late_completion_resolution`: `Option<LateCompletionResolutionClass>`
- `late_completion_detail`: `Option<String>`
- `stale_completion_arrivals`: `u64`

## Inputs
- `PlanOccurrence`(occurrence_id: OccurrenceId, schedule_id: ScheduleId, schedule_revision: u64, occurrence_ordinal: u64, trigger_key: TriggerKey, target_binding_key: TargetBindingId, misfire_policy: MisfirePolicy, misfire_policy_key: String, overlap_policy: OverlapPolicy, overlap_policy_key: String, missing_target_policy: MissingTargetPolicy, missing_target_policy_key: String, target_materialized_session_id: Option<SessionId>, due_at_utc_ms: u64, misfire_deadline_utc_ms: u64)
- `SyncTargetSnapshot`(target_binding_key: TargetBindingId, target_materialized_session_id: Option<SessionId>)
- `RecordReceipt`(correlation_id: Option<CorrelationId>, detail: Option<String>, materialized_session_id: Option<SessionId>, runtime_outcome_key: Option<RuntimeOutcomeKey>)
- `ClassifyDue`(now_utc_ms: u64)
- `ClassifyOccurrenceTerminality`
- `ClassifyClaimedDispatchDisposition`(schedule_phase: ClaimedDispatchSchedulePhase, current_schedule_revision: u64)
- `ClassifyCompletionSupersession`(schedule_phase: ClaimedDispatchSchedulePhase, current_schedule_revision: u64)
- `Claim`(owner_id: ClaimOwner, at_utc_ms: u64, lease_expires_at_utc_ms: u64, claim_token: ClaimToken)
- `DispatchStarted`(correlation_id: Option<CorrelationId>, at_utc_ms: u64)
- `AwaitCompletion`(at_utc_ms: u64)
- `Complete`(at_utc_ms: u64)
- `ResolveRuntimeCompletion`(outcome: RuntimeCompletionOutcome, detail: Option<String>, at_utc_ms: u64)
- `ResolveDeliveryCompletionFailure`(reason: DeliveryCompletionFailureReason, detail: Option<String>, at_utc_ms: u64)
- `ResolveDeliveryFailure`(reason: DeliveryFailureReason, detail: Option<String>, at_utc_ms: u64)
- `ResolveTargetProbe`(outcome: OccurrenceTargetProbeOutcome, detail: Option<String>, at_utc_ms: u64)
- `ResolveDueMisfire`(detail: Option<String>, at_utc_ms: u64)
- `Supersede`(superseded_by_revision: u64, at_utc_ms: u64)
- `LeaseExpired`(at_utc_ms: u64)
- `ReleaseLeaseForPausedSchedule`(at_utc_ms: u64)
- `ClassifyTransitionFailure`(refusal_kind: OccurrenceTransitionFailureRefusalKind, trigger: OccurrenceLifecycleInputVariant)
- `ClassifyStaleCompletionArrival`(trigger: OccurrenceLifecycleInputVariant)

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
- `OccurrenceTerminalityClassified`(terminal: Bool)
- `ClaimedDispatchDispositionClassified`(disposition: ClaimedDispatchDisposition, superseded_by_revision: Option<u64>)
- `CompletionSupersessionClassified`(disposition: CompletionSupersessionDisposition, superseded_by_revision: Option<u64>)
- `DeliveryFailed`
- `LeaseExpired`
- `TransitionFailureClassified`(phase: OccurrenceLifecycleState, refusal_kind: OccurrenceTransitionFailureRefusalKind, trigger: OccurrenceLifecycleInputVariant, public_class: OccurrenceTransitionFailureClassKind)
- `LateCompletionResolutionRecorded`(resolution: LateCompletionResolutionClass)
- `StaleCompletionArrivalClassified`(phase: OccurrenceLifecycleState, trigger: OccurrenceLifecycleInputVariant)

## Helpers
- `is_live_claim_phase`(phase: OccurrenceLifecycleState) -> `Bool`

## Invariants
- `live_claim_requires_owner`
- `superseded_records_revision`
- `delivery_failed_records_failure_class`
- `misfire_deadline_not_before_due`
- `late_completion_only_after_supersession`
- `late_completion_resolution_requires_timestamp`
- `late_completion_timestamp_requires_resolution`
- `late_completion_detail_requires_resolution`

## Transitions
### `ClassifyTransitionFailurePlanRejectedPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `plan_rejected`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailurePlanRejectedClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `plan_rejected`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailurePlanRejectedDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `plan_rejected`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailurePlanRejectedAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `plan_rejected`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailurePlanRejectedCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `plan_rejected`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailurePlanRejectedSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `plan_rejected`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailurePlanRejectedMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `plan_rejected`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailurePlanRejectedSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `plan_rejected`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailurePlanRejectedDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `plan_rejected`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureTargetSyncRejectedPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `target_sync_rejected`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureTargetSyncRejectedClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `target_sync_rejected`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureTargetSyncRejectedDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `target_sync_rejected`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `target_sync_rejected`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureTargetSyncRejectedCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `target_sync_rejected`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureTargetSyncRejectedSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `target_sync_rejected`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureTargetSyncRejectedMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `target_sync_rejected`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureTargetSyncRejectedSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `target_sync_rejected`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `target_sync_rejected`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureReceiptRecordRejectedPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `receipt_record_rejected`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureReceiptRecordRejectedClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `receipt_record_rejected`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureReceiptRecordRejectedDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `receipt_record_rejected`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `receipt_record_rejected`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureReceiptRecordRejectedCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `receipt_record_rejected`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureReceiptRecordRejectedSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `receipt_record_rejected`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureReceiptRecordRejectedMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `receipt_record_rejected`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureReceiptRecordRejectedSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `receipt_record_rejected`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `receipt_record_rejected`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureDueClassificationRejectedPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `due_classification_rejected`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureDueClassificationRejectedClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `due_classification_rejected`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureDueClassificationRejectedDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `due_classification_rejected`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `due_classification_rejected`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureDueClassificationRejectedCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `due_classification_rejected`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureDueClassificationRejectedSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `due_classification_rejected`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureDueClassificationRejectedMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `due_classification_rejected`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureDueClassificationRejectedSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `due_classification_rejected`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `due_classification_rejected`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureClaimedDispatchDispositionRejectedPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `claimed_dispatch_disposition_rejected`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureClaimedDispatchDispositionRejectedClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `claimed_dispatch_disposition_rejected`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureClaimedDispatchDispositionRejectedDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `claimed_dispatch_disposition_rejected`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureClaimedDispatchDispositionRejectedAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `claimed_dispatch_disposition_rejected`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureClaimedDispatchDispositionRejectedCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `claimed_dispatch_disposition_rejected`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureClaimedDispatchDispositionRejectedSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `claimed_dispatch_disposition_rejected`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureClaimedDispatchDispositionRejectedMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `claimed_dispatch_disposition_rejected`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureClaimedDispatchDispositionRejectedSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `claimed_dispatch_disposition_rejected`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureClaimedDispatchDispositionRejectedDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `claimed_dispatch_disposition_rejected`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureCompletionSupersessionRejectedPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `completion_supersession_rejected`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureCompletionSupersessionRejectedClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `completion_supersession_rejected`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureCompletionSupersessionRejectedDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `completion_supersession_rejected`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureCompletionSupersessionRejectedAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `completion_supersession_rejected`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureCompletionSupersessionRejectedCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `completion_supersession_rejected`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureCompletionSupersessionRejectedSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `completion_supersession_rejected`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureCompletionSupersessionRejectedMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `completion_supersession_rejected`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureCompletionSupersessionRejectedSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `completion_supersession_rejected`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureCompletionSupersessionRejectedDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `completion_supersession_rejected`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureClaimRejectedPendingPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `claim_rejected_pending`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureNotPendingForClaimClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_pending_for_claim`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureNotPendingForClaimDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_pending_for_claim`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_pending_for_claim`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureNotPendingForClaimCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_pending_for_claim`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureNotPendingForClaimSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_pending_for_claim`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureNotPendingForClaimMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_pending_for_claim`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureNotPendingForClaimSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_pending_for_claim`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureNotPendingForClaimDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_pending_for_claim`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureNotClaimedPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_claimed`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureNotClaimedClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_claimed`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureNotClaimedDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_claimed`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureNotClaimedAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_claimed`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureNotClaimedCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_claimed`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureNotClaimedSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_claimed`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureNotClaimedMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_claimed`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureNotClaimedSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_claimed`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureNotClaimedDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_claimed`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureNotDispatchingPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_dispatching`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureNotDispatchingClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_dispatching`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureNotDispatchingDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_dispatching`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureNotDispatchingAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_dispatching`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureNotDispatchingCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_dispatching`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureNotDispatchingSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_dispatching`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureNotDispatchingMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_dispatching`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureNotDispatchingSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_dispatching`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureNotDispatchingDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_dispatching`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureNotLeaseHoldingPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_lease_holding`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureNotLeaseHoldingClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_lease_holding`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureNotLeaseHoldingDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_lease_holding`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_lease_holding`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureNotLeaseHoldingCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_lease_holding`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureNotLeaseHoldingSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_lease_holding`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureNotLeaseHoldingMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_lease_holding`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureNotLeaseHoldingSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_lease_holding`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_lease_holding`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureNotLiveForTerminalPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_live_for_terminal`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureNotLiveForTerminalClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_live_for_terminal`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureNotLiveForTerminalDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_live_for_terminal`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_live_for_terminal`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureNotLiveForTerminalCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_live_for_terminal`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureNotLiveForTerminalSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_live_for_terminal`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureNotLiveForTerminalMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_live_for_terminal`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureNotLiveForTerminalSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_live_for_terminal`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `not_live_for_terminal`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyTransitionFailureStaleCompletionArrivalRejectedPending`
- From: `Pending`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `stale_completion_arrival_rejected`
- Emits: `TransitionFailureClassified`
- To: `Pending`

### `ClassifyTransitionFailureStaleCompletionArrivalRejectedClaimed`
- From: `Claimed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `stale_completion_arrival_rejected`
- Emits: `TransitionFailureClassified`
- To: `Claimed`

### `ClassifyTransitionFailureStaleCompletionArrivalRejectedDispatching`
- From: `Dispatching`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `stale_completion_arrival_rejected`
- Emits: `TransitionFailureClassified`
- To: `Dispatching`

### `ClassifyTransitionFailureStaleCompletionArrivalRejectedAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `stale_completion_arrival_rejected`
- Emits: `TransitionFailureClassified`
- To: `AwaitingCompletion`

### `ClassifyTransitionFailureStaleCompletionArrivalRejectedCompleted`
- From: `Completed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `stale_completion_arrival_rejected`
- Emits: `TransitionFailureClassified`
- To: `Completed`

### `ClassifyTransitionFailureStaleCompletionArrivalRejectedSkipped`
- From: `Skipped`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `stale_completion_arrival_rejected`
- Emits: `TransitionFailureClassified`
- To: `Skipped`

### `ClassifyTransitionFailureStaleCompletionArrivalRejectedMisfired`
- From: `Misfired`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `stale_completion_arrival_rejected`
- Emits: `TransitionFailureClassified`
- To: `Misfired`

### `ClassifyTransitionFailureStaleCompletionArrivalRejectedSuperseded`
- From: `Superseded`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `stale_completion_arrival_rejected`
- Emits: `TransitionFailureClassified`
- To: `Superseded`

### `ClassifyTransitionFailureStaleCompletionArrivalRejectedDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyTransitionFailure`(refusal_kind, trigger)
- Guards:
  - `stale_completion_arrival_rejected`
- Emits: `TransitionFailureClassified`
- To: `DeliveryFailed`

### `ClassifyStaleCompletionArrivalObservedPending`
- From: `Pending`
- On: `ClassifyStaleCompletionArrival`(trigger)
- Guards:
  - `completion_shaped_trigger`
- Emits: `StaleCompletionArrivalClassified`
- To: `Pending`

### `ClassifyStaleCompletionArrivalObservedClaimed`
- From: `Claimed`
- On: `ClassifyStaleCompletionArrival`(trigger)
- Guards:
  - `completion_shaped_trigger`
- Emits: `StaleCompletionArrivalClassified`
- To: `Claimed`

### `ClassifyStaleCompletionArrivalObservedDispatching`
- From: `Dispatching`
- On: `ClassifyStaleCompletionArrival`(trigger)
- Guards:
  - `completion_shaped_trigger`
- Emits: `StaleCompletionArrivalClassified`
- To: `Dispatching`

### `ClassifyStaleCompletionArrivalObservedAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyStaleCompletionArrival`(trigger)
- Guards:
  - `completion_shaped_trigger`
- Emits: `StaleCompletionArrivalClassified`
- To: `AwaitingCompletion`

### `ClassifyStaleCompletionArrivalObservedCompleted`
- From: `Completed`
- On: `ClassifyStaleCompletionArrival`(trigger)
- Guards:
  - `completion_shaped_trigger`
- Emits: `StaleCompletionArrivalClassified`
- To: `Completed`

### `ClassifyStaleCompletionArrivalObservedSkipped`
- From: `Skipped`
- On: `ClassifyStaleCompletionArrival`(trigger)
- Guards:
  - `completion_shaped_trigger`
- Emits: `StaleCompletionArrivalClassified`
- To: `Skipped`

### `ClassifyStaleCompletionArrivalObservedMisfired`
- From: `Misfired`
- On: `ClassifyStaleCompletionArrival`(trigger)
- Guards:
  - `completion_shaped_trigger`
- Emits: `StaleCompletionArrivalClassified`
- To: `Misfired`

### `ClassifyStaleCompletionArrivalObservedSuperseded`
- From: `Superseded`
- On: `ClassifyStaleCompletionArrival`(trigger)
- Guards:
  - `completion_shaped_trigger`
- Emits: `StaleCompletionArrivalClassified`
- To: `Superseded`

### `ClassifyStaleCompletionArrivalObservedDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyStaleCompletionArrival`(trigger)
- Guards:
  - `completion_shaped_trigger`
- Emits: `StaleCompletionArrivalClassified`
- To: `DeliveryFailed`

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

### `ClassifyOccurrenceTerminalityTerminalCompleted`
- From: `Completed`
- On: `ClassifyOccurrenceTerminality`()
- Emits: `OccurrenceTerminalityClassified`
- To: `Completed`

### `ClassifyOccurrenceTerminalityTerminalSkipped`
- From: `Skipped`
- On: `ClassifyOccurrenceTerminality`()
- Emits: `OccurrenceTerminalityClassified`
- To: `Skipped`

### `ClassifyOccurrenceTerminalityTerminalMisfired`
- From: `Misfired`
- On: `ClassifyOccurrenceTerminality`()
- Emits: `OccurrenceTerminalityClassified`
- To: `Misfired`

### `ClassifyOccurrenceTerminalityTerminalSuperseded`
- From: `Superseded`
- On: `ClassifyOccurrenceTerminality`()
- Emits: `OccurrenceTerminalityClassified`
- To: `Superseded`

### `ClassifyOccurrenceTerminalityTerminalDeliveryFailed`
- From: `DeliveryFailed`
- On: `ClassifyOccurrenceTerminality`()
- Emits: `OccurrenceTerminalityClassified`
- To: `DeliveryFailed`

### `ClassifyOccurrenceTerminalityLivePending`
- From: `Pending`
- On: `ClassifyOccurrenceTerminality`()
- Emits: `OccurrenceTerminalityClassified`
- To: `Pending`

### `ClassifyOccurrenceTerminalityLiveClaimed`
- From: `Claimed`
- On: `ClassifyOccurrenceTerminality`()
- Emits: `OccurrenceTerminalityClassified`
- To: `Claimed`

### `ClassifyOccurrenceTerminalityLiveDispatching`
- From: `Dispatching`
- On: `ClassifyOccurrenceTerminality`()
- Emits: `OccurrenceTerminalityClassified`
- To: `Dispatching`

### `ClassifyOccurrenceTerminalityLiveAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `ClassifyOccurrenceTerminality`()
- Emits: `OccurrenceTerminalityClassified`
- To: `AwaitingCompletion`

### `ClassifyClaimedDispatchDispositionFutureRevision`
- From: `Claimed`
- On: `ClassifyClaimedDispatchDisposition`(schedule_phase, current_schedule_revision)
- Guards:
  - ``
- Emits: `ClaimedDispatchDispositionClassified`
- To: `Claimed`

### `ClassifyClaimedDispatchDispositionFrozen`
- From: `Claimed`
- On: `ClassifyClaimedDispatchDisposition`(schedule_phase, current_schedule_revision)
- Guards:
  - ``
- Emits: `ClaimedDispatchDispositionClassified`
- To: `Claimed`

### `ClassifyClaimedDispatchDispositionSupersedeDeleted`
- From: `Claimed`
- On: `ClassifyClaimedDispatchDisposition`(schedule_phase, current_schedule_revision)
- Guards:
  - ``
- Emits: `ClaimedDispatchDispositionClassified`
- To: `Claimed`

### `ClassifyClaimedDispatchDispositionSupersedeStale`
- From: `Claimed`
- On: `ClassifyClaimedDispatchDisposition`(schedule_phase, current_schedule_revision)
- Guards:
  - ``
- Emits: `ClaimedDispatchDispositionClassified`
- To: `Claimed`

### `ClassifyClaimedDispatchDispositionReady`
- From: `Claimed`
- On: `ClassifyClaimedDispatchDisposition`(schedule_phase, current_schedule_revision)
- Guards:
  - ``
- Emits: `ClaimedDispatchDispositionClassified`
- To: `Claimed`

### `ClassifyCompletionSupersessionDeleted`
- From: `AwaitingCompletion`
- On: `ClassifyCompletionSupersession`(schedule_phase, current_schedule_revision)
- Guards:
  - ``
- Emits: `CompletionSupersessionClassified`
- To: `AwaitingCompletion`

### `ClassifyCompletionSupersessionStale`
- From: `AwaitingCompletion`
- On: `ClassifyCompletionSupersession`(schedule_phase, current_schedule_revision)
- Guards:
  - ``
- Emits: `CompletionSupersessionClassified`
- To: `AwaitingCompletion`

### `ClassifyCompletionSupersessionProceed`
- From: `AwaitingCompletion`
- On: `ClassifyCompletionSupersession`(schedule_phase, current_schedule_revision)
- Guards:
  - ``
- Emits: `CompletionSupersessionClassified`
- To: `AwaitingCompletion`

### `ClassifyCompletionSupersessionAlreadySuperseded`
- From: `Superseded`
- On: `ClassifyCompletionSupersession`(schedule_phase, current_schedule_revision)
- Emits: `CompletionSupersessionClassified`
- To: `Superseded`

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

### `AwaitCompletionAfterSupersession`
- From: `Superseded`
- On: `AwaitCompletion`(at_utc_ms)
- To: `Superseded`

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

### `DeliveryCompletionFailureTransportError`
- From: `Dispatching`, `AwaitingCompletion`
- On: `ResolveDeliveryCompletionFailure`(reason, detail, at_utc_ms)
- Guards:
  - `completion_future_failed`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `DeliveryCompletionFailureInternalError`
- From: `Dispatching`, `AwaitingCompletion`
- On: `ResolveDeliveryCompletionFailure`(reason, detail, at_utc_ms)
- Guards:
  - `runtime_completion_authority_absent`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `DeliveryFailureTargetMaterializationFailed`
- From: `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `ResolveDeliveryFailure`(reason, detail, at_utc_ms)
- Guards:
  - `target_materialization_failed`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `DeliveryFailureTargetMissing`
- From: `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `ResolveDeliveryFailure`(reason, detail, at_utc_ms)
- Guards:
  - `target_missing`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `DeliveryFailureTargetBusy`
- From: `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `ResolveDeliveryFailure`(reason, detail, at_utc_ms)
- Guards:
  - `target_busy`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `DeliveryFailureRuntimeRejected`
- From: `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `ResolveDeliveryFailure`(reason, detail, at_utc_ms)
- Guards:
  - `runtime_rejected`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `DeliveryFailureMobRejected`
- From: `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `ResolveDeliveryFailure`(reason, detail, at_utc_ms)
- Guards:
  - `mob_rejected`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `DeliveryFailureTransportError`
- From: `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `ResolveDeliveryFailure`(reason, detail, at_utc_ms)
- Guards:
  - `transport_error`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `DeliveryFailureInternalError`
- From: `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `ResolveDeliveryFailure`(reason, detail, at_utc_ms)
- Guards:
  - `internal_error`
- Emits: `DeliveryFailed`
- To: `DeliveryFailed`

### `TargetProbeReadyClaimed`
- From: `Claimed`
- On: `ResolveTargetProbe`(outcome, detail, at_utc_ms)
- Guards:
  - `ready`
- To: `Claimed`

### `TargetProbeBusyAllowedByPolicy`
- From: `Claimed`
- On: `ResolveTargetProbe`(outcome, detail, at_utc_ms)
- Guards:
  - `busy`
  - `allow_concurrent`
- To: `Claimed`

### `TargetProbeBusySkipByPolicy`
- From: `Claimed`
- On: `ResolveTargetProbe`(outcome, detail, at_utc_ms)
- Guards:
  - `busy`
  - `skip_if_running`
- Emits: `Skipped`
- To: `Skipped`

### `TargetProbeMissingSkipByPolicy`
- From: `Claimed`
- On: `ResolveTargetProbe`(outcome, detail, at_utc_ms)
- Guards:
  - `missing`
  - `skip_missing_target`
- Emits: `Skipped`
- To: `Skipped`

### `TargetProbeMissingMisfireByPolicy`
- From: `Claimed`
- On: `ResolveTargetProbe`(outcome, detail, at_utc_ms)
- Guards:
  - `missing`
  - `mark_misfired_missing_target`
- Emits: `Misfired`
- To: `Misfired`

### `DueMisfirePending`
- From: `Pending`
- On: `ResolveDueMisfire`(detail, at_utc_ms)
- Guards:
  - ``
- Emits: `Misfired`
- To: `Misfired`

### `SupersedePendingOrLive`
- From: `Pending`, `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `Supersede`(superseded_by_revision, at_utc_ms)
- Emits: `Superseded`, `OccurrencesSuperseded`
- To: `Superseded`

### `SupersedeAlreadySuperseded`
- From: `Superseded`
- On: `Supersede`(superseded_by_revision, at_utc_ms)
- To: `Superseded`

### `LateCompleteAfterSupersession`
- From: `Superseded`
- On: `Complete`(at_utc_ms)
- Emits: `LateCompletionResolutionRecorded`
- To: `Superseded`

### `LateRuntimeCompletionCompletedAfterSupersession`
- From: `Superseded`
- On: `ResolveRuntimeCompletion`(outcome, detail, at_utc_ms)
- Guards:
  - `runtime_outcome_completed`
- Emits: `LateCompletionResolutionRecorded`
- To: `Superseded`

### `LateRuntimeCompletionRejectedAfterSupersession`
- From: `Superseded`
- On: `ResolveRuntimeCompletion`(outcome, detail, at_utc_ms)
- Guards:
  - `runtime_outcome_rejected`
- Emits: `LateCompletionResolutionRecorded`
- To: `Superseded`

### `LateRuntimeCompletionTransportErrorAfterSupersession`
- From: `Superseded`
- On: `ResolveRuntimeCompletion`(outcome, detail, at_utc_ms)
- Guards:
  - `runtime_outcome_transport_error`
- Emits: `LateCompletionResolutionRecorded`
- To: `Superseded`

### `LateRuntimeCompletionInternalErrorAfterSupersession`
- From: `Superseded`
- On: `ResolveRuntimeCompletion`(outcome, detail, at_utc_ms)
- Guards:
  - `runtime_outcome_internal_error`
- Emits: `LateCompletionResolutionRecorded`
- To: `Superseded`

### `LateDeliveryCompletionFailureAfterSupersession`
- From: `Superseded`
- On: `ResolveDeliveryCompletionFailure`(reason, detail, at_utc_ms)
- Emits: `LateCompletionResolutionRecorded`
- To: `Superseded`

### `LateDeliveryFailureAfterSupersession`
- From: `Superseded`
- On: `ResolveDeliveryFailure`(reason, detail, at_utc_ms)
- Emits: `LateCompletionResolutionRecorded`
- To: `Superseded`

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

### `ReleaseLeaseForPausedScheduleAfterSupersession`
- From: `Superseded`
- On: `ReleaseLeaseForPausedSchedule`(at_utc_ms)
- To: `Superseded`

## Coverage
### Code Anchors
- `occurrence_lifecycle` (machine `OccurrenceLifecycleMachine`): `meerkat-schedule/src/lifecycle.rs` — Occurrence::planned_from_schedule and Occurrence::apply domain-facing lifecycle transition seam over plan occurrence from pending, sync target snapshot from pending or claimed materialized bindings, record receipt from pending, claimed, dispatching, awaiting completion, completed, skipped, misfired, superseded, or delivery failed result projection, classify due no action, due claim eligible, due misfire required, due lease expired, claim, claimed, dispatch, await completion, complete, resolve runtime completion outcome, completed, skip, skipped, misfire, misfired, supersede, superseded, delivery failure, lease expiry, live owner, revision, and failure classification

### Scenarios
- `occurrence_start_complete_fail` — occurrence transitions through pending, running, and terminal lifecycle states
- `occurrence_claim_dispatch_completion` — plan occurrence from pending, sync target snapshot from pending or claimed materialized bindings, record receipt from pending, claimed, dispatching, awaiting completion, completed, skipped, misfired, superseded, or delivery failed result projection, claim pending occurrence, dispatch started from claimed, await completion, complete from dispatching or awaiting, resolve runtime completion outcome, and record claimed/dispatch/awaiting/completed effects
- `occurrence_terminal_classification` — skip/skipped, misfire/misfired, supersede/superseded, delivery failed, occurrences superseded, records revision and explicit failure class for terminal occurrence outcomes
- `occurrence_lease_recovery` — classify due no action, due claim eligible, due misfire required, due lease expired, and lease expired from claimed, dispatching, or awaiting completion returns live claimed work to owner-aware recovery
