# OccurrenceLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `OccurrenceLifecycleMachine`

### Code Anchors
- `occurrence_authority`: `meerkat-schedule/src/authority.rs` — occurrence lifecycle authority that owns claim, dispatch, lease expiry, and terminal outcomes
- `schedule_driver`: `meerkat-schedule/src/driver.rs` — mechanical scheduler driver precursor for due claims, probes, dispatch, and feedback
- `schedule_sqlite_store`: `meerkat-store/src/schedule_sqlite_store.rs` — sqlite schedule store precursor for authoritative claim-time and durable lease state

### Scenarios
- `claim-dispatch-complete` — pending occurrence claims, dispatches, awaits owner feedback, and completes
- `lease-expiry-reclaim` — claimed or dispatching occurrence returns to claimable after lease expiry
- `supersede-and-fail` — occurrence terminalizes as superseded or delivery_failed with explicit failure class

### Transitions
- `ClaimPending`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `StartRuntimeDispatchFromClaimed`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `StartMobDispatchFromClaimed`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `RuntimeAcceptedFromDispatching`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `MobAcceptedFromDispatching`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `RuntimeCompletedFromDispatchingOrAwaiting`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `MobCompletedFromDispatchingOrAwaiting`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `RuntimeSkippedFromDispatchingOrAwaiting`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `MobSkippedFromDispatchingOrAwaiting`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `RuntimeMisfiredFromDispatchingOrAwaiting`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `MobMisfiredFromDispatchingOrAwaiting`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `RuntimeDeliveryFailedFromDispatchingOrAwaiting`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `MobDeliveryFailedFromDispatchingOrAwaiting`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `SupersedePending`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `LeaseExpiredFromClaimed`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `LeaseExpiredFromDispatching`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `LeaseExpiredFromAwaitingCompletion`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`

### Effects
- `ClaimLeaseGranted`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `DispatchToRuntime`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `DispatchToMob`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `AppendReceipt`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `EmitOccurrenceNotice`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`

### Invariants
- `terminal_has_no_open_delivery_protocol`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `live_claim_requires_lease_holder`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `awaiting_completion_requires_protocol`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `superseded_records_revision`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`
- `delivery_failed_records_failure_class`
  - anchors: `occurrence_authority`, `schedule_driver`, `schedule_sqlite_store`
  - scenarios: `claim-dispatch-complete`


<!-- GENERATED_COVERAGE_END -->
