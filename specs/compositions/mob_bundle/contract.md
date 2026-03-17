# mob_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `mob_lifecycle`: `MobLifecycleMachine` @ actor `mob_lifecycle_actor`
- `mob_orchestrator`: `MobOrchestratorMachine` @ actor `mob_orchestrator_actor`
- `flow_run`: `FlowRunMachine` @ actor `flow_engine`
- `ops_lifecycle`: `OpsLifecycleMachine` @ actor `ops_plane`
- `peer_comms`: `PeerCommsMachine` @ actor `peer_plane`
- `runtime_control`: `RuntimeControlMachine` @ actor `control_plane`
- `runtime_ingress`: `RuntimeIngressMachine` @ actor `ordinary_ingress`
- `turn_execution`: `TurnExecutionMachine` @ actor `turn_executor`

## Routes
- `flow_step_dispatch_enters_runtime_admission`: `flow_run`.`AdmitStepWork` -> `runtime_control`.`SubmitWork` [Immediate]
- `mob_async_op_event_enters_runtime_admission`: `ops_lifecycle`.`SubmitOpEvent` -> `runtime_control`.`SubmitWork` [Immediate]
- `mob_peer_candidate_enters_runtime_admission`: `peer_comms`.`SubmitPeerInputCandidate` -> `runtime_control`.`SubmitWork` [Immediate]
- `mob_admitted_work_enters_ingress`: `runtime_control`.`SubmitAdmittedIngressEffect` -> `runtime_ingress`.`AdmitQueued` [Immediate]
- `mob_ingress_ready_starts_runtime_control`: `runtime_ingress`.`ReadyForRun` -> `runtime_control`.`BeginRun` [Immediate]
- `mob_runtime_control_starts_execution`: `runtime_control`.`SubmitRunPrimitive` -> `turn_execution`.`StartConversationRun` [Immediate]
- `mob_execution_boundary_updates_ingress`: `turn_execution`.`BoundaryApplied` -> `runtime_ingress`.`BoundaryApplied` [Immediate]
- `mob_execution_completion_updates_ingress`: `turn_execution`.`RunCompleted` -> `runtime_ingress`.`RunCompleted` [Immediate]
- `mob_execution_completion_notifies_control`: `turn_execution`.`RunCompleted` -> `runtime_control`.`RunCompleted` [Immediate]
- `mob_execution_failure_updates_ingress`: `turn_execution`.`RunFailed` -> `runtime_ingress`.`RunFailed` [Immediate]
- `mob_execution_failure_notifies_control`: `turn_execution`.`RunFailed` -> `runtime_control`.`RunFailed` [Immediate]
- `mob_execution_cancel_updates_ingress`: `turn_execution`.`RunCancelled` -> `runtime_ingress`.`RunCancelled` [Immediate]
- `mob_execution_cancel_notifies_control`: `turn_execution`.`RunCancelled` -> `runtime_control`.`RunCancelled` [Immediate]

## Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`

## Structural Requirements
- `control_preempts_mob_ingress` — runtime control outranks mob-side ingress work when both are ready

## Behavioral Invariants
- `flow_dispatch_uses_canonical_runtime_admission` — flow-run step dispatch reaches runtime only through the runtime-control admission surface
- `mob_async_lifecycle_events_use_operation_input` — mob-backed async lifecycle events re-enter runtime through the operation-input admission path
- `mob_peer_work_uses_canonical_runtime_admission` — member peer communication enters runtime only through canonical admission
- `mob_runtime_work_flows_into_ingress` — mob-originated admitted work is handed into canonical ingress ownership
- `mob_execution_failure_is_handled` — mob turn-execution failure is handled by both ingress and runtime control
- `mob_execution_cancel_is_handled` — mob turn-execution cancellation is handled by both ingress and runtime control

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/actor.rs` — mob orchestration precursor
- `meerkat-mob/src/runtime/handle.rs` — member-directed delivery capability
- `meerkat-mob/src/runtime/flow.rs` — flow dispatch precursor
- `meerkat-comms/src/runtime/comms_runtime.rs` — mob member peer communication precursor
- `examples/031-wasm-mini-diplomacy-sh/web/src/main.ts` — mob-based WASM example coverage
- `examples/032-wasm-webcm-agent/web/src/main.ts` — browser mob workflow coverage
- `examples/033-the-office-demo-sh/web/src/main.ts` — browser local-tool + mob coverage

### Scenarios
- `mob-flow-dispatch` — flow step work enters runtime only through canonical admission
- `mob-child-report-back` — mob-backed child work reports progress and terminality through ops lifecycle
- `mob-peer-orchestration` — member-directed communication and orchestration stay inside the mob stack
- `wasm-mob-examples` — browser mob examples continue to fit the canonical mob/comms/runtime model
