# OccurrenceLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `OccurrenceLifecycleMachine`

### Code Anchors
- `occurrence_lifecycle`: `meerkat-schedule/src/lifecycle.rs` — Occurrence::planned_from_schedule and Occurrence::apply domain-facing lifecycle transition seam over plan occurrence from pending, sync target snapshot from pending or claimed materialized bindings, record receipt from pending, claimed, dispatching, awaiting completion, completed, skipped, misfired, superseded, or delivery failed result projection, classify due no action, due claim eligible, due misfire required, due lease expired, claim, claimed, dispatch, await completion, complete, completed, skip, skipped, misfire, misfired, supersede, superseded, delivery failure, lease expiry, live owner, revision, and failure classification

### Scenarios
- `occurrence_start_complete_fail` — occurrence transitions through pending, running, and terminal lifecycle states
- `occurrence_claim_dispatch_completion` — plan occurrence from pending, sync target snapshot from pending or claimed materialized bindings, record receipt from pending, claimed, dispatching, awaiting completion, completed, skipped, misfired, superseded, or delivery failed result projection, claim pending occurrence, dispatch started from claimed, await completion, complete from dispatching or awaiting, and record claimed/dispatch/awaiting/completed effects
- `occurrence_terminal_classification` — skip/skipped, misfire/misfired, supersede/superseded, delivery failed, occurrences superseded, records revision and explicit failure class for terminal occurrence outcomes
- `occurrence_lease_recovery` — classify due no action, due claim eligible, due misfire required, due lease expired, and lease expired from claimed, dispatching, or awaiting completion returns live claimed work to owner-aware recovery

### Transitions
- `PlanOccurrenceFromPending`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `ClassifyDuePendingFuture`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDuePendingMisfire`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDuePendingClaimEligible`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueClaimedLeaseExpired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueDispatchingLeaseExpired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueAwaitingCompletionLeaseExpired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueClaimedLeaseCurrent`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueDispatchingLeaseCurrent`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueAwaitingCompletionLeaseCurrent`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueCompletedNoAction`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueSkippedNoAction`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueMisfiredNoAction`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueSupersededNoAction`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `ClassifyDueDeliveryFailedNoAction`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`, `occurrence_terminal_classification`, `occurrence_lease_recovery`
- `SyncTargetSnapshotPending`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `SyncTargetSnapshotClaimed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `RecordReceiptPending`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `RecordReceiptClaimed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `RecordReceiptDispatching`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `RecordReceiptAwaitingCompletion`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `RecordReceiptCompleted`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `RecordReceiptSkipped`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `RecordReceiptMisfired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `RecordReceiptSuperseded`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
- `RecordReceiptDeliveryFailed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
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
  - scenarios: `occurrence_claim_dispatch_completion`
- `DeliveryFailedFromClaimedOrLive`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`
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
  - scenarios: `occurrence_claim_dispatch_completion`, `occurrence_terminal_classification`
- `Misfired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`, `occurrence_terminal_classification`
- `Superseded`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`, `occurrence_terminal_classification`
- `OccurrencesSuperseded`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_terminal_classification`
- `DueNoAction`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `DueClaimEligible`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `DueMisfireRequired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `DueLeaseExpired`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`
- `DeliveryFailed`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_claim_dispatch_completion`, `occurrence_terminal_classification`
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
- `misfire_deadline_not_before_due`
  - anchors: `occurrence_lifecycle`
  - scenarios: `occurrence_lease_recovery`


<!-- GENERATED_COVERAGE_END -->
