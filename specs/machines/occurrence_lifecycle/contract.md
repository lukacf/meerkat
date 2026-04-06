# OccurrenceLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-schedule` / `generated::occurrence_lifecycle`

## State
- Phase enum: `Pending | Claimed | Dispatching | AwaitingCompletion | Completed | Skipped | Misfired | Superseded | DeliveryFailed`
- `occurrence_id`: `OccurrenceId`
- `schedule_id`: `ScheduleId`
- `schedule_revision`: `u64`
- `occurrence_ordinal`: `u64`
- `target_binding_key`: `String`
- `due_at_utc_ms`: `u64`
- `claimed_by`: `Option<String>`
- `lease_expires_at_utc_ms`: `Option<u64>`
- `claimed_at_utc_ms`: `Option<u64>`
- `claim_token`: `Option<String>`
- `delivery_correlation_id`: `Option<String>`
- `last_receipt_stage`: `Option<DeliveryReceiptStage>`
- `failure_class`: `Option<OccurrenceFailureClass>`
- `failure_detail`: `Option<String>`
- `dispatched_at_utc_ms`: `Option<u64>`
- `completed_at_utc_ms`: `Option<u64>`
- `attempt_count`: `u64`
- `superseded_by_revision`: `Option<u64>`

## Inputs
- `Claim`(owner_id: String, at_utc_ms: u64, lease_expires_at_utc_ms: u64, claim_token: String)
- `DispatchStarted`(correlation_id: Option<String>, at_utc_ms: u64)
- `AwaitCompletion`(at_utc_ms: u64)
- `Complete`(receipt_stage: DeliveryReceiptStage, at_utc_ms: u64)
- `Skip`(detail: Option<String>, failure_class: Option<OccurrenceFailureClass>, at_utc_ms: u64)
- `Misfire`(detail: Option<String>, failure_class: Option<OccurrenceFailureClass>, at_utc_ms: u64)
- `Supersede`(superseded_by_revision: u64, at_utc_ms: u64)
- `DeliveryFailed`(receipt_stage: Option<DeliveryReceiptStage>, failure_class: OccurrenceFailureClass, detail: Option<String>, at_utc_ms: u64)
- `LeaseExpired`(at_utc_ms: u64)

## Effects
- `Claimed`
- `DispatchStarted`
- `AwaitingCompletion`
- `Completed`
- `Skipped`
- `Misfired`
- `Superseded`
- `DeliveryFailed`
- `LeaseExpired`

## Helpers
- `is_live_claim_phase`(phase: OccurrenceLifecycleState) -> `Bool`

## Invariants
- `live_claim_requires_owner`
- `superseded_records_revision`
- `delivery_failed_records_failure_class`

## Transitions
### `ClaimPending`
- From: `Pending`
- On: `Claim`(owner_id, at_utc_ms, lease_expires_at_utc_ms, claim_token)
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
- On: `Complete`(receipt_stage, at_utc_ms)
- Emits: `Completed`
- To: `Completed`

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
- Emits: `Superseded`
- To: `Superseded`

### `DeliveryFailedFromClaimedOrLive`
- From: `Claimed`, `Dispatching`, `AwaitingCompletion`
- On: `DeliveryFailed`(receipt_stage, failure_class, detail, at_utc_ms)
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

## Coverage
### Code Anchors
- `meerkat-schedule/src/authority.rs` — occurrence lifecycle authority that owns claim, dispatch, lease expiry, and terminal outcomes
- `meerkat-schedule/src/driver.rs` — mechanical scheduler driver precursor for due claims, probes, dispatch, and feedback
- `meerkat-schedule/src/store.rs` — durable claim-time and occurrence state precursor
- `meerkat-machine-schema/src/catalog/occurrence_lifecycle.rs` — formal OccurrenceLifecycleMachine schema

### Scenarios
- `occurrence-claim-dispatch-complete` — occurrences claim, dispatch, and reach a terminal outcome with attempt ownership preserved
- `occurrence-supersede` — pending occurrences supersede when a newer schedule revision invalidates them
- `occurrence-lease-expiry` — live claimed work returns to pending when a lease expires before completion
