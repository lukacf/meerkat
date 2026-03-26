# ops_peer_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `ops_peer_bundle`

### Code Anchors
- `ops_lifecycle_shell`: `meerkat-runtime/src/ops_lifecycle.rs` — ops lifecycle shell that handles ExposeOperationPeer effect
- `comms_runtime`: `meerkat-comms/src/runtime/comms_runtime.rs` — add_trusted_peer wiring from ops to peer comms

### Scenarios
- `peer-ready-handoff` — ops-lifecycle PeerReady triggers peer-comms trust establishment

### Routes
- `ops_peer_ready_trusts_peer_comms`
  - anchors: `ops_lifecycle_shell`, `comms_runtime`
  - scenarios: `peer-ready-handoff`

### Scheduler Rules
- `(none)`

### Invariants
- `ops_peer_ready_trusts_peer_comms`
  - anchors: `ops_lifecycle_shell`, `comms_runtime`
  - scenarios: `peer-ready-handoff`


<!-- GENERATED_COVERAGE_END -->
