# OccurrenceLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `OccurrenceLifecycleMachine`

### Code Anchors
- `occurrence_lifecycle`: `meerkat-schedule/src/lifecycle.rs` — Occurrence::apply domain-facing lifecycle transition seam over the DSL

### Scenarios
- `occurrence_start_complete_fail` — occurrence transitions through pending, running, and terminal lifecycle states

### Transitions
- `ClaimPending`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `DispatchStartedFromClaimed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `AwaitCompletionFromDispatching`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `CompleteFromDispatchingOrAwaiting`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `SkipFromPendingOrLive`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `MisfireFromPendingOrLive`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `SupersedePendingOrLive`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `DeliveryFailedFromClaimedOrLive`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `LeaseExpiredFromClaimed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `LeaseExpiredFromDispatching`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `LeaseExpiredFromAwaitingCompletion`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`

### Effects
- `Claimed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `DispatchStarted`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `AwaitingCompletion`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `Completed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `Skipped`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `Misfired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `Superseded`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `DeliveryFailed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `LeaseExpired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`

### Invariants
- `live_claim_requires_owner`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `superseded_records_revision`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`
- `delivery_failed_records_failure_class`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`


<!-- GENERATED_COVERAGE_END -->
