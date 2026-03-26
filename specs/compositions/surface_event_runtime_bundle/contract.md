# surface_event_runtime_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `runtime_control`: `RuntimeControlMachine` @ actor `control_plane`
- `runtime_ingress`: `RuntimeIngressMachine` @ actor `ordinary_ingress`
- `turn_execution`: `TurnExecutionMachine` @ actor `turn_executor`

## Routes
- `surface_event_enters_ingress`: `runtime_control`.`SubmitAdmittedIngressEffect` -> `runtime_ingress`.`AdmitQueued` [Immediate]
- `surface_ingress_ready_starts_runtime_control`: `runtime_ingress`.`ReadyForRun` -> `runtime_control`.`BeginRun` [Immediate]
- `surface_runtime_control_starts_execution`: `runtime_control`.`SubmitRunPrimitive` -> `turn_execution`.`StartConversationRun` [Immediate]
- `surface_execution_boundary_updates_ingress`: `turn_execution`.`BoundaryApplied` -> `runtime_ingress`.`BoundaryApplied` [Immediate]
- `surface_execution_completion_updates_ingress`: `turn_execution`.`RunCompleted` -> `runtime_ingress`.`RunCompleted` [Immediate]
- `surface_execution_completion_notifies_control`: `turn_execution`.`RunCompleted` -> `runtime_control`.`RunCompleted` [Immediate]
- `surface_execution_failure_updates_ingress`: `turn_execution`.`RunFailed` -> `runtime_ingress`.`RunFailed` [Immediate]
- `surface_execution_failure_notifies_control`: `turn_execution`.`RunFailed` -> `runtime_control`.`RunFailed` [Immediate]
- `surface_execution_cancel_updates_ingress`: `turn_execution`.`RunCancelled` -> `runtime_ingress`.`RunCancelled` [Immediate]
- `surface_execution_cancel_notifies_control`: `turn_execution`.`RunCancelled` -> `runtime_control`.`RunCancelled` [Immediate]

## Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`

## Structural Requirements
- `control_preempts_surface_event_ingress` — runtime control outranks surface event ingress when both are ready

## Behavioral Invariants
- `surface_event_uses_canonical_runtime_admission` — surface external events enter runtime ingress only through runtime-control admission
- `surface_event_begin_run_requires_staged_drain` — surface-originated work begins a run only after ingress-owned staging
- `surface_event_execution_completion_is_handled` — surface-originated execution completion updates both ingress and runtime control
- `surface_event_execution_failure_is_handled` — surface-originated execution failure updates both ingress and runtime control
- `surface_event_execution_cancel_is_handled` — surface-originated execution cancellation updates both ingress and runtime control

## Coverage
### Code Anchors
- `meerkat-cli/src/stdin_events.rs` — CLI external-event ingestion precursor
- `meerkat-rest/src/lib.rs` — REST external-event surface precursor
- `meerkat-rpc/src/handlers/event.rs` — JSON-RPC external-event surface precursor
- `meerkat-web-runtime/src/lib.rs` — WASM/browser external-event surface precursor
- `meerkat-runtime/src/runtime_loop.rs` — canonical external-event surface routing (input_to_append, input_to_prompt)

### Scenarios
- `cli-surface-event-admission` — CLI stdin and host-driven external events use canonical runtime admission
- `rest-surface-event-admission` — REST external events use canonical runtime admission
- `rpc-surface-event-admission` — JSON-RPC external events use canonical runtime admission
- `wasm-surface-event-admission` — browser/WASM external events use canonical runtime admission
- `surface-event-failure` — surface-originated runs still route failure through ingress and control
- `surface-control-preemption` — control-plane work still preempts surface-originated ingress
