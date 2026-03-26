# comms_drain_lifecycle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `comms_drain`: `CommsDrainLifecycleMachine` @ actor `drain_plane`

## Routes

## Scheduler Rules
- `(none)`

## Structural Requirements
- `spawn_protocol_covered` — SpawnDrainTask effect is covered by comms_drain_spawn handoff protocol
- `abort_protocol_covered` — AbortDrainTask effect is covered by comms_drain_abort handoff protocol

## Behavioral Invariants
- `(none)`

## Coverage
### Code Anchors
- `meerkat-core/src/comms_drain_lifecycle_authority.rs` — comms drain lifecycle authority (sealed mutator + evaluate)
- `meerkat-core/src/generated/protocol_comms_drain_spawn.rs` — generated spawn protocol helper for comms drain handoff
- `meerkat-core/src/agent/comms_impl.rs` — comms drain shell implementation (effect realization + feedback)

### Scenarios
- `spawn-feedback-cycle` — SpawnDrainTask obligation is closed by TaskSpawned or TaskExited feedback
- `abort-terminal-closure` — AbortDrainTask obligation closes on terminal phase without explicit feedback
