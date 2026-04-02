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
- `lease_held`: `Bool`
- `lease_owner`: `String`
- `lease_expiry_utc_ms`: `u64`
- `delivery_correlation_id`: `Option<String>`
- `open_delivery_protocol`: `Option<OccurrenceDeliveryProtocol>`
- `last_receipt_stage`: `Option<DeliveryReceiptStage>`
- `failure_class`: `Option<OccurrenceFailureClass>`
- `attempt_count`: `u64`
- `superseded_by_revision`: `Option<u64>`

## Inputs
- `Claim`(claim_time_utc_ms: u64, owner_id: String, lease_expiry_utc_ms: u64)
- `StartRuntimeDispatch`(delivery_correlation_id: String)
- `StartMobDispatch`(delivery_correlation_id: String)
- `RuntimeAccepted`(occurrence_id: OccurrenceId, attempt_count: u64)
- `MobAccepted`(occurrence_id: OccurrenceId, attempt_count: u64)
- `RuntimeCompleted`(occurrence_id: OccurrenceId, attempt_count: u64)
- `MobCompleted`(occurrence_id: OccurrenceId, attempt_count: u64)
- `RuntimeSkipped`(occurrence_id: OccurrenceId, attempt_count: u64, failure_class: OccurrenceFailureClass)
- `MobSkipped`(occurrence_id: OccurrenceId, attempt_count: u64, failure_class: OccurrenceFailureClass)
- `RuntimeMisfired`(occurrence_id: OccurrenceId, attempt_count: u64, failure_class: OccurrenceFailureClass)
- `MobMisfired`(occurrence_id: OccurrenceId, attempt_count: u64, failure_class: OccurrenceFailureClass)
- `RuntimeDeliveryFailed`(occurrence_id: OccurrenceId, attempt_count: u64, failure_class: OccurrenceFailureClass)
- `MobDeliveryFailed`(occurrence_id: OccurrenceId, attempt_count: u64, failure_class: OccurrenceFailureClass)
- `SupersedeByRevision`(superseding_revision: u64)
- `LeaseExpired`

## Effects
- `ClaimLeaseGranted`(occurrence_id: OccurrenceId, owner_id: String, lease_expiry_utc_ms: u64, attempt_count: u64)
- `DispatchToRuntime`(occurrence_id: OccurrenceId, schedule_id: ScheduleId, schedule_revision: u64, occurrence_ordinal: u64, attempt_count: u64, target_binding_key: String, delivery_correlation_id: Option<String>)
- `DispatchToMob`(occurrence_id: OccurrenceId, schedule_id: ScheduleId, schedule_revision: u64, occurrence_ordinal: u64, attempt_count: u64, target_binding_key: String, delivery_correlation_id: Option<String>)
- `AppendReceipt`(stage: DeliveryReceiptStage)
- `EmitOccurrenceNotice`(new_state: OccurrenceLifecycleState)

## Helpers
- `claimable_at`(store_now_utc_ms: u64) -> `Bool`
- `phase_is_terminal`(phase: OccurrenceLifecycleState) -> `Bool`

## Invariants
- `terminal_has_no_open_delivery_protocol`
- `live_claim_requires_lease_holder`
- `awaiting_completion_requires_protocol`
- `superseded_records_revision`
- `delivery_failed_records_failure_class`

## Transitions
### `ClaimPending`
- From: `Pending`
- On: `Claim`(claim_time_utc_ms, owner_id, lease_expiry_utc_ms)
- Guards:
  - `claimable_at_store_time`
- Emits: `ClaimLeaseGranted`, `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Claimed`

### `StartRuntimeDispatchFromClaimed`
- From: `Claimed`
- On: `StartRuntimeDispatch`(delivery_correlation_id)
- Emits: `DispatchToRuntime`, `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Dispatching`

### `StartMobDispatchFromClaimed`
- From: `Claimed`
- On: `StartMobDispatch`(delivery_correlation_id)
- Emits: `DispatchToMob`, `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Dispatching`

### `RuntimeAcceptedFromDispatching`
- From: `Dispatching`
- On: `RuntimeAccepted`(occurrence_id, attempt_count)
- Guards:
  - `matching_protocol_is_open`
  - `occurrence_identity_matches`
  - `attempt_matches`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `AwaitingCompletion`

### `MobAcceptedFromDispatching`
- From: `Dispatching`
- On: `MobAccepted`(occurrence_id, attempt_count)
- Guards:
  - `matching_protocol_is_open`
  - `occurrence_identity_matches`
  - `attempt_matches`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `AwaitingCompletion`

### `RuntimeCompletedFromDispatchingOrAwaiting`
- From: `Dispatching`, `AwaitingCompletion`
- On: `RuntimeCompleted`(occurrence_id, attempt_count)
- Guards:
  - `matching_protocol_is_open`
  - `occurrence_identity_matches`
  - `attempt_matches`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Completed`

### `MobCompletedFromDispatchingOrAwaiting`
- From: `Dispatching`, `AwaitingCompletion`
- On: `MobCompleted`(occurrence_id, attempt_count)
- Guards:
  - `matching_protocol_is_open`
  - `occurrence_identity_matches`
  - `attempt_matches`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Completed`

### `RuntimeSkippedFromDispatchingOrAwaiting`
- From: `Dispatching`, `AwaitingCompletion`
- On: `RuntimeSkipped`(occurrence_id, attempt_count, failure_class)
- Guards:
  - `matching_protocol_is_open`
  - `occurrence_identity_matches`
  - `attempt_matches`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Skipped`

### `MobSkippedFromDispatchingOrAwaiting`
- From: `Dispatching`, `AwaitingCompletion`
- On: `MobSkipped`(occurrence_id, attempt_count, failure_class)
- Guards:
  - `matching_protocol_is_open`
  - `occurrence_identity_matches`
  - `attempt_matches`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Skipped`

### `RuntimeMisfiredFromDispatchingOrAwaiting`
- From: `Dispatching`, `AwaitingCompletion`
- On: `RuntimeMisfired`(occurrence_id, attempt_count, failure_class)
- Guards:
  - `matching_protocol_is_open`
  - `occurrence_identity_matches`
  - `attempt_matches`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Misfired`

### `MobMisfiredFromDispatchingOrAwaiting`
- From: `Dispatching`, `AwaitingCompletion`
- On: `MobMisfired`(occurrence_id, attempt_count, failure_class)
- Guards:
  - `matching_protocol_is_open`
  - `occurrence_identity_matches`
  - `attempt_matches`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Misfired`

### `RuntimeDeliveryFailedFromDispatchingOrAwaiting`
- From: `Dispatching`, `AwaitingCompletion`
- On: `RuntimeDeliveryFailed`(occurrence_id, attempt_count, failure_class)
- Guards:
  - `matching_protocol_is_open`
  - `occurrence_identity_matches`
  - `attempt_matches`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `DeliveryFailed`

### `MobDeliveryFailedFromDispatchingOrAwaiting`
- From: `Dispatching`, `AwaitingCompletion`
- On: `MobDeliveryFailed`(occurrence_id, attempt_count, failure_class)
- Guards:
  - `matching_protocol_is_open`
  - `occurrence_identity_matches`
  - `attempt_matches`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `DeliveryFailed`

### `SupersedePending`
- From: `Pending`
- On: `SupersedeByRevision`(superseding_revision)
- Guards:
  - `superseding_revision_is_newer`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Superseded`

### `LeaseExpiredFromClaimed`
- From: `Claimed`
- On: `LeaseExpired`()
- Guards:
  - `lease_was_held`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Pending`

### `LeaseExpiredFromDispatching`
- From: `Dispatching`
- On: `LeaseExpired`()
- Guards:
  - `lease_was_held`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Pending`

### `LeaseExpiredFromAwaitingCompletion`
- From: `AwaitingCompletion`
- On: `LeaseExpired`()
- Guards:
  - `lease_was_held`
- Emits: `AppendReceipt`, `EmitOccurrenceNotice`
- To: `Pending`

## Coverage
### Code Anchors
- `meerkat-schedule/src/authority.rs` — occurrence lifecycle authority that owns claim, dispatch, lease expiry, and terminal outcomes
- `meerkat-schedule/src/driver.rs` — mechanical scheduler driver precursor for due claims, probes, dispatch, and feedback
- `meerkat-store/src/schedule_sqlite_store.rs` — sqlite schedule store precursor for authoritative claim-time and durable lease state

### Scenarios
- `claim-dispatch-complete` — pending occurrence claims, dispatches, awaits owner feedback, and completes
- `lease-expiry-reclaim` — claimed or dispatching occurrence returns to claimable after lease expiry
- `supersede-and-fail` — occurrence terminalizes as superseded or delivery_failed with explicit failure class
