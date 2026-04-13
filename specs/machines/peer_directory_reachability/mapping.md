# PeerDirectoryReachabilityMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `PeerDirectoryReachabilityMachine`

### Code Anchors
- `peer_directory_reachability_authority`: `meerkat-comms/src/peer_directory_reachability_authority.rs` — peer directory reachability authority and transition ownership

### Scenarios
- `peer_reachability_probe` — reachability probes transition peer directory membership across probe outcomes

### Transitions
- `ReconcileResolvedDirectory`
  - anchors: `peer_directory_reachability_authority`
  - scenarios: `peer_reachability_probe`
- `RecordSendSucceeded`
  - anchors: `peer_directory_reachability_authority`
  - scenarios: `peer_reachability_probe`
- `RecordSendFailed`
  - anchors: `peer_directory_reachability_authority`
  - scenarios: `peer_reachability_probe`

### Effects
- `(none)`

### Invariants
- `reachability_keys_are_resolved`
  - anchors: `peer_directory_reachability_authority`
  - scenarios: `peer_reachability_probe`
- `last_reason_keys_are_resolved`
  - anchors: `peer_directory_reachability_authority`
  - scenarios: `peer_reachability_probe`


<!-- GENERATED_COVERAGE_END -->
