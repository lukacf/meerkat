# comms_drain_lifecycle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `comms_drain_lifecycle`

### Code Anchors
- `comms_drain_authority`: `meerkat-core/src/comms_drain_lifecycle_authority.rs` — comms drain lifecycle authority (sealed mutator + evaluate)
- `comms_drain_protocol`: `meerkat-core/src/generated/protocol_comms_drain_spawn.rs` — generated spawn protocol helper for comms drain handoff
- `comms_drain_impl`: `meerkat-core/src/agent/comms_impl.rs` — comms drain shell implementation (effect realization + feedback)

### Scenarios
- `spawn-feedback-cycle` — SpawnDrainTask obligation is closed by TaskSpawned or TaskExited feedback
- `abort-terminal-closure` — AbortDrainTask obligation closes on terminal phase without explicit feedback

### Routes
- `(none)`

### Scheduler Rules
- `(none)`

### Invariants
- `spawn_protocol_covered`
  - anchors: `comms_drain_authority`, `comms_drain_protocol`, `comms_drain_impl`
  - scenarios: `spawn-feedback-cycle`
- `abort_protocol_covered`
  - anchors: `comms_drain_authority`, `comms_drain_protocol`, `comms_drain_impl`
  - scenarios: `abort-terminal-closure`


<!-- GENERATED_COVERAGE_END -->
