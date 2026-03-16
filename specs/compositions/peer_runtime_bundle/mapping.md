# peer_runtime_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `peer_runtime_bundle`

### Code Anchors
- `peer_classify`: `meerkat-comms/src/classify.rs` — peer normalization precursor
- `peer_runtime`: `meerkat-comms/src/runtime/comms_runtime.rs` — runtime-facing peer delivery precursor
- `runtime_loop`: `meerkat-runtime/src/runtime_loop.rs` — runtime admission/control precursor

### Scenarios
- `peer-message-admission` — peer-classified work reaches runtime only through canonical admission
- `trust-before-admission` — trust/classification is fixed before runtime sees peer work
- `no-direct-host-bypass` — peer work does not bypass the runtime path

### Routes
- `peer_candidate_enters_runtime_admission`
  - anchors: `peer_classify`, `peer_runtime`, `runtime_loop`
  - scenarios: `peer-message-admission`, `trust-before-admission`
- `admitted_peer_work_enters_ingress`
  - anchors: `peer_classify`, `peer_runtime`, `runtime_loop`
  - scenarios: `peer-message-admission`, `trust-before-admission`

### Scheduler Rules
- `PreemptWhenReady(control_plane, peer_plane)`
  - anchors: `peer_classify`, `peer_runtime`, `runtime_loop`
  - scenarios: `no-direct-host-bypass`

### Invariants
- `peer_work_enters_runtime_via_canonical_admission`
  - anchors: `peer_classify`, `peer_runtime`, `runtime_loop`
  - scenarios: `peer-message-admission`, `trust-before-admission`
- `peer_admission_flows_into_ingress`
  - anchors: `peer_classify`, `peer_runtime`, `runtime_loop`
  - scenarios: `peer-message-admission`, `trust-before-admission`
- `control_preempts_peer_delivery`
  - anchors: `peer_classify`, `peer_runtime`, `runtime_loop`
  - scenarios: `no-direct-host-bypass`


<!-- GENERATED_COVERAGE_END -->
