# ops_peer_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `ops_lifecycle`: `OpsLifecycleMachine` @ actor `ops_plane`
- `peer_comms`: `PeerCommsMachine` @ actor `peer_plane`

## Routes
- `ops_peer_ready_trusts_peer_comms`: `ops_lifecycle`.`ExposeOperationPeer` -> `peer_comms`.`TrustPeer` [Immediate]

## Scheduler Rules
- `(none)`

## Structural Requirements
- `(none)`

## Behavioral Invariants
- `ops_peer_ready_trusts_peer_comms` — ops-lifecycle peer-ready effect triggers peer-comms trust establishment through an explicit route

## Coverage
### Code Anchors
- `meerkat-runtime/src/ops_lifecycle.rs` — ops lifecycle shell that handles ExposeOperationPeer effect
- `meerkat-comms/src/runtime/comms_runtime.rs` — add_trusted_peer wiring from ops to peer comms

### Scenarios
- `peer-ready-handoff` — ops-lifecycle PeerReady triggers peer-comms trust establishment
