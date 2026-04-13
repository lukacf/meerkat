# OccurrenceLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `OccurrenceLifecycleMachine`

### Code Anchors
- `occurrence_authority`: `meerkat-schedule/src/authority.rs` — occurrence lifecycle authority

### Scenarios
- `occurrence_start_complete_fail` — occurrence transitions through pending, running, and terminal lifecycle states

### Transitions
- `ClaimPending`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `DispatchStartedFromClaimed`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `AwaitCompletionFromDispatching`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `CompleteFromDispatchingOrAwaiting`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `SkipFromPendingOrLive`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `MisfireFromPendingOrLive`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `SupersedePendingOrLive`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `DeliveryFailedFromClaimedOrLive`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `LeaseExpiredFromClaimed`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `LeaseExpiredFromDispatching`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `LeaseExpiredFromAwaitingCompletion`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`

### Effects
- `Claimed`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `DispatchStarted`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `AwaitingCompletion`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `Completed`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `Skipped`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `Misfired`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `Superseded`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `DeliveryFailed`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `LeaseExpired`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`

### Invariants
- `live_claim_requires_owner`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `superseded_records_revision`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`
- `delivery_failed_records_failure_class`
  - anchors: `occurrence_authority`
  - scenarios: `occurrence_start_complete_fail`


<!-- GENERATED_COVERAGE_END -->
