# OccurrenceLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `OccurrenceLifecycleMachine`

### Code Anchors
- `occurrence_lifecycle`: `meerkat-schedule/src/lifecycle.rs` — Occurrence::apply domain-facing lifecycle transition seam over claim, claimed, dispatch, await completion, complete, completed, skip, skipped, misfire, misfired, supersede, superseded, delivery failure, lease expiry, live owner, revision, and failure classification

### Scenarios
- `occurrence_start_complete_fail` — occurrence transitions through pending, running, and terminal lifecycle states
- `occurrence_claim_dispatch_completion` — claim pending occurrence, dispatch started from claimed, await completion, complete from dispatching or awaiting, and record claimed/dispatch/awaiting/completed effects
- `occurrence_terminal_classification` — skip/skipped, misfire/misfired, supersede/superseded, delivery failed, occurrences superseded, records revision and explicit failure class for terminal occurrence outcomes
- `occurrence_lease_recovery` — lease expired from claimed, dispatching, or awaiting completion returns live claimed work to owner-aware recovery

### Transitions
- `ClaimPending`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `DispatchStartedFromClaimed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `AwaitCompletionFromDispatching`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`, `occurrence_lease_recovery`
- `CompleteFromDispatchingOrAwaiting`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `SkipFromPendingOrLive`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `MisfireFromPendingOrLive`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `SupersedePendingOrLive`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_start_complete_fail`, `occurrence_claim_dispatch_completion`, `occurrence_terminal_classification`
- `DeliveryFailedFromClaimedOrLive`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`, `occurrence_terminal_classification`, `occurrence_lease_recovery`
- `LeaseExpiredFromClaimed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `LeaseExpiredFromDispatching`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `LeaseExpiredFromAwaitingCompletion`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`

### Effects
- `Claimed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`, `occurrence_lease_recovery`
- `DispatchStarted`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `AwaitingCompletion`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`, `occurrence_lease_recovery`
- `Completed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `Skipped`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_terminal_classification`
- `Misfired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_terminal_classification`
- `Superseded`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_terminal_classification`
- `OccurrencesSuperseded`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_terminal_classification`
- `DeliveryFailed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_terminal_classification`
- `LeaseExpired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`

### Invariants
- `live_claim_requires_owner`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `superseded_records_revision`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_terminal_classification`
- `delivery_failed_records_failure_class`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_terminal_classification`


<!-- GENERATED_COVERAGE_END -->
