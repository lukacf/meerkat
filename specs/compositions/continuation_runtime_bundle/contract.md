# continuation_runtime_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `runtime_control`: `RuntimeControlMachine` @ actor `control_plane`
- `runtime_ingress`: `RuntimeIngressMachine` @ actor `ordinary_ingress`
- `turn_execution`: `TurnExecutionMachine` @ actor `turn_executor`

## Routes
- `continuation_enters_ingress`: `runtime_control`.`SubmitAdmittedIngressEffect` -> `runtime_ingress`.`AdmitQueued` [Immediate]
- `continuation_ingress_ready_starts_runtime_control`: `runtime_ingress`.`ReadyForRun` -> `runtime_control`.`BeginRun` [Immediate]
- `continuation_runtime_control_starts_execution`: `runtime_control`.`SubmitRunPrimitive` -> `turn_execution`.`StartConversationRun` [Immediate]
- `continuation_execution_completion_updates_ingress`: `turn_execution`.`RunCompleted` -> `runtime_ingress`.`RunCompleted` [Immediate]
- `continuation_execution_completion_notifies_control`: `turn_execution`.`RunCompleted` -> `runtime_control`.`RunCompleted` [Immediate]
- `continuation_execution_boundary_updates_ingress`: `turn_execution`.`BoundaryApplied` -> `runtime_ingress`.`BoundaryApplied` [Immediate]
- `continuation_execution_failure_updates_ingress`: `turn_execution`.`RunFailed` -> `runtime_ingress`.`RunFailed` [Immediate]
- `continuation_execution_failure_notifies_control`: `turn_execution`.`RunFailed` -> `runtime_control`.`RunFailed` [Immediate]
- `continuation_execution_cancel_updates_ingress`: `turn_execution`.`RunCancelled` -> `runtime_ingress`.`RunCancelled` [Immediate]
- `continuation_execution_cancel_notifies_control`: `turn_execution`.`RunCancelled` -> `runtime_control`.`RunCancelled` [Immediate]

## Scheduler Rules
- `PreemptWhenReady(control_plane, ordinary_ingress)`

## Structural Requirements
- `control_preempts_continuation_ingress` — runtime control outranks continuation ingress when both are ready

## Behavioral Invariants
- `continuation_uses_canonical_runtime_admission` — runtime-owned continuation work enters ingress only through runtime-control admission
- `continuation_begin_run_requires_staged_drain` — continuation work begins a run only after ingress-owned staging
- `continuation_execution_completion_is_handled` — continuation execution completion updates both ingress and runtime control
- `continuation_execution_failure_is_handled` — continuation execution failure updates both ingress and runtime control
- `continuation_execution_cancel_is_handled` — continuation execution cancellation updates both ingress and runtime control

## Coverage
### Code Anchors
- `docs/architecture/0.5/meerkat_host_mode_cutover_spec.md` — runtime-owned continuation scheduling contract
- `meerkat-runtime/src/comms_drain.rs` — comms inbox drain feeding typed inputs into the runtime adapter
- `meerkat-core/src/agent/comms_impl.rs` — terminal peer response continuation precursor
- `meerkat-core/src/agent/runner.rs` — continuation acceptance precursor

### Scenarios
- `terminal-response-continuation` — terminal peer responses schedule continuation through runtime-owned admission
- `host-mode-continuation` — host-mode continuation still runs through the canonical runtime path
- `continuation-control-preemption` — control-plane work preempts continuation ingress scheduling
