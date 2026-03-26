# runtime_pipeline

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `runtime_control`: `RuntimeControlMachine` @ actor `control_plane`
- `runtime_ingress`: `RuntimeIngressMachine` @ actor `ordinary_ingress`
- `turn_execution`: `TurnExecutionMachine` @ actor `turn_executor`

## Routes
- `admitted_work_enters_ingress`: `runtime_control`.`SubmitAdmittedIngressEffect` -> `runtime_ingress`.`AdmitQueued` [Immediate]
- `staged_run_notifies_control`: `runtime_ingress`.`ReadyForRun` -> `runtime_control`.`BeginRun` [Immediate]
- `control_starts_execution`: `runtime_control`.`SubmitRunPrimitive` -> `turn_execution`.`StartConversationRun` [Immediate]
- `execution_boundary_updates_ingress`: `turn_execution`.`BoundaryApplied` -> `runtime_ingress`.`BoundaryApplied` [Immediate]
- `execution_completion_updates_ingress`: `turn_execution`.`RunCompleted` -> `runtime_ingress`.`RunCompleted` [Immediate]
- `execution_completion_notifies_control`: `turn_execution`.`RunCompleted` -> `runtime_control`.`RunCompleted` [Immediate]
- `execution_failure_updates_ingress`: `turn_execution`.`RunFailed` -> `runtime_ingress`.`RunFailed` [Immediate]
- `execution_failure_notifies_control`: `turn_execution`.`RunFailed` -> `runtime_control`.`RunFailed` [Immediate]
- `execution_cancel_updates_ingress`: `turn_execution`.`RunCancelled` -> `runtime_ingress`.`RunCancelled` [Immediate]
- `execution_cancel_notifies_control`: `turn_execution`.`RunCancelled` -> `runtime_control`.`RunCancelled` [Immediate]

## Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`

## Structural Requirements
- `control_preempts_ordinary_work` — control_plane outranks ordinary_ingress whenever both are ready

## Behavioral Invariants
- `admitted_work_reaches_ingress_through_runtime_control` — runtime-admitted ordinary work reaches ingress only through the runtime-control admission handoff
- `begin_run_requires_staged_drain` — runtime control may begin a run only from ingress-owned staged work
- `begun_run_must_start_execution` — a begun run must flow into turn execution through the canonical primitive handoff
- `execution_failure_is_handled` — turn-execution failure must route into ingress rollback and runtime lifecycle handling
- `execution_cancel_is_handled` — turn-execution cancellation must route into ingress rollback and runtime lifecycle handling
- `execution_completion_is_handled` — turn-execution completion must route into ingress consumption and runtime lifecycle handling

## Coverage
### Code Anchors
- `meerkat-runtime/src/runtime_loop.rs` — runtime orchestration precursor for control/ingress/execution
- `meerkat-core/src/lifecycle/core_executor.rs` — turn execution bridge precursor
- `meerkat-core/src/lifecycle/run_event.rs` — boundary/completion effect surface precursor

### Scenarios
- `prompt-queue` — queued prompt stays ordinary and waits for the next normal boundary when a run is already active
- `prompt-steer` — steering prompt requests ASAP admission-to-ingress handling while preserving ordinary-work semantics
- `runtime-success-path` — staged work begins a run, applies a boundary, and completes
- `runtime-failure-rollback` — failed run rolls staged contributors back before steady state
- `runtime-cancel-rollback` — cancelled run rolls staged contributors back before steady state
- `control-preemption` — control-plane work preempts ordinary ingress scheduling
