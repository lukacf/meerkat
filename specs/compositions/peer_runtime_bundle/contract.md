# peer_runtime_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `peer_comms`: `PeerCommsMachine` @ actor `peer_plane`
- `runtime_control`: `RuntimeControlMachine` @ actor `control_plane`
- `runtime_ingress`: `RuntimeIngressMachine` @ actor `ordinary_ingress`

## Routes
- `peer_candidate_enters_runtime_admission`: `peer_comms`.`SubmitPeerInputCandidate` -> `runtime_control`.`SubmitCandidate` [Immediate]
- `admitted_peer_work_enters_ingress`: `runtime_control`.`SubmitAdmittedIngressEffect` -> `runtime_ingress`.`AdmitQueued` [Immediate]

## Scheduler Rules
- `PreemptWhenReady(control_plane, peer_plane)`

## Structural Requirements
- `control_preempts_peer_delivery` — runtime control outranks peer delivery once both planes are ready

## Behavioral Invariants
- `peer_work_enters_runtime_via_canonical_admission` — peer-classified work enters the runtime through the runtime-control admission surface
- `peer_admission_flows_into_ingress` — peer-admitted work is handed from runtime control into canonical ingress ownership

## Coverage
### Code Anchors
- `meerkat-comms/src/classify.rs` — peer normalization precursor
- `meerkat-comms/src/runtime/comms_runtime.rs` — runtime-facing peer delivery precursor
- `meerkat-runtime/src/runtime_loop.rs` — runtime admission/control precursor

### Scenarios
- `peer-message-admission` — peer-classified work reaches runtime only through canonical admission
- `trust-before-admission` — trust/classification is fixed before runtime sees peer work
- `no-direct-host-bypass` — peer work does not bypass the runtime path
