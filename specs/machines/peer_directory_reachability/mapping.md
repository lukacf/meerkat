# PeerDirectoryReachabilityMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `PeerDirectoryReachabilityMachine`

### Code Anchors
- `peer_directory_reachability_authority`: `meerkat-comms/src/peer_directory_reachability_authority.rs` — canonical transient reachability authority and reconcile/send-result reducer
- `comms_runtime_directory_projection`: `meerkat-comms/src/runtime/comms_runtime.rs` — runtime-owned resolved peer directory projection and send-result integration

### Scenarios
- `reconcile-directory` — resolved peer directory snapshot is reconciled into transient reachability state
- `send-success-marks-reachable` — successful delivery marks an already resolved peer reachable
- `send-failure-marks-unreachable` — offline or transport failures mark a resolved peer unreachable without inventing unknown peers

### Transitions
- `ReconcileResolvedDirectory`
  - anchors: `peer_directory_reachability_authority`, `comms_runtime_directory_projection`
  - scenarios: `reconcile-directory`
- `RecordSendSucceeded`
  - anchors: `peer_directory_reachability_authority`, `comms_runtime_directory_projection`
  - scenarios: `send-success-marks-reachable`, `send-failure-marks-unreachable`
- `RecordSendFailed`
  - anchors: `peer_directory_reachability_authority`, `comms_runtime_directory_projection`
  - scenarios: `send-success-marks-reachable`, `send-failure-marks-unreachable`

### Effects
- `(none)`

### Invariants
- `reachability_keys_are_resolved`
  - anchors: `peer_directory_reachability_authority`, `comms_runtime_directory_projection`
  - scenarios: `send-success-marks-reachable`, `send-failure-marks-unreachable`
- `last_reason_keys_are_resolved`
  - anchors: `peer_directory_reachability_authority`, `comms_runtime_directory_projection`
  - scenarios: `send-success-marks-reachable`, `send-failure-marks-unreachable`


<!-- GENERATED_COVERAGE_END -->
