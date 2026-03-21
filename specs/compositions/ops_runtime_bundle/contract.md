# ops_runtime_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `ops_lifecycle`: `OpsLifecycleMachine` @ actor `ops_plane`
- `runtime_control`: `RuntimeControlMachine` @ actor `control_plane`
- `runtime_ingress`: `RuntimeIngressMachine` @ actor `ordinary_ingress`
- `turn_execution`: `TurnExecutionMachine` @ actor `turn_executor`

## Routes
- `op_event_enters_runtime_admission`: `ops_lifecycle`.`SubmitOpEvent` -> `runtime_control`.`SubmitWork` [Immediate]
- `admitted_op_work_enters_ingress`: `runtime_control`.`SubmitAdmittedIngressEffect` -> `runtime_ingress`.`AdmitQueued` [Immediate]
- `ingress_ready_starts_runtime_control`: `runtime_ingress`.`ReadyForRun` -> `runtime_control`.`BeginRun` [Immediate]
- `runtime_control_starts_execution`: `runtime_control`.`SubmitRunPrimitive` -> `turn_execution`.`StartConversationRun` [Immediate]
- `execution_boundary_updates_ingress`: `turn_execution`.`BoundaryApplied` -> `runtime_ingress`.`BoundaryApplied` [Immediate]
- `execution_completion_updates_ingress`: `turn_execution`.`RunCompleted` -> `runtime_ingress`.`RunCompleted` [Immediate]
- `execution_completion_notifies_control`: `turn_execution`.`RunCompleted` -> `runtime_control`.`RunCompleted` [Immediate]
- `execution_failure_updates_ingress`: `turn_execution`.`RunFailed` -> `runtime_ingress`.`RunFailed` [Immediate]
- `execution_failure_notifies_control`: `turn_execution`.`RunFailed` -> `runtime_control`.`RunFailed` [Immediate]
- `execution_cancel_updates_ingress`: `turn_execution`.`RunCancelled` -> `runtime_ingress`.`RunCancelled` [Immediate]
- `execution_cancel_notifies_control`: `turn_execution`.`RunCancelled` -> `runtime_control`.`RunCancelled` [Immediate]
- `ops_barrier_satisfied_enters_turn_execution`: `ops_lifecycle`.`WaitAllSatisfied` -> `turn_execution`.`OpsBarrierSatisfied` [Immediate]

## Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`

## Structural Requirements
- `barrier_satisfaction_handoff_protocol_covered` — OpsLifecycle WaitAllSatisfied effect is covered by the ops_barrier_satisfaction handoff protocol, routing barrier satisfaction to TurnExecution

## Behavioral Invariants
- `async_op_events_reenter_runtime_via_operation_input` — async-operation lifecycle events re-enter runtime through the canonical operation-input admission surface
- `admitted_op_work_flows_into_ingress` — runtime-admitted operation work is handed into canonical ingress ownership
- `op_execution_failure_is_handled` — turn-execution failure after operation admission is handled by both ingress and runtime control
- `op_execution_cancel_is_handled` — turn-execution cancellation after operation admission is handled by both ingress and runtime control

## Coverage
### Code Anchors
- `meerkat-core/src/ops.rs` — shared async-operation vocabulary precursor
- `meerkat-runtime/src/runtime_loop.rs` — runtime operation admission precursor
- `meerkat-tools/src/builtin/shell/job_manager.rs` — background async-op source precursor

### Scenarios
- `operation-event-reentry` — async-operation events re-enter runtime as operation input
- `ops-runtime-terminality` — lifecycle and runtime both observe required terminal outcomes
- `ops-control-preemption` — control-plane work still outranks ops-driven ingress
