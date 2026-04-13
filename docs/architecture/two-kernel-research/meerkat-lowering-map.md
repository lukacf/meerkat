# Meerkat Lowering Map

Status: frozen exact-current-state asset

This note closes `K8` from `meerkat-cutover-checklist.md`.

It maps the exact current `MeerkatMachine` inputs/effects to real functions and
code paths in the current codebase.

The goal is not to claim that every lower-level authority helper is part of the
top-level boundary. The goal is to make the actual live lowerings explicit.

## Binding / control lowering

| Machine input/effect | Current lowering |
| --- | --- |
| `PrepareBindings` | `MeerkatMachine::prepare_bindings(...)` |
| `RegisterSession` | `MeerkatMachine::register_session(...)` |
| `RegisterSessionWithExecutor` | `MeerkatMachine::register_session_with_executor(...)` |
| `InterruptCurrentRun` | `MeerkatMachine::interrupt_current_run(...)` |
| `ExecutorControl(CancelCurrentRun)` | attached runtime `control_tx.send(RunControlCommand::CancelCurrentRun { .. })` in `meerkat_machine.rs` |
| `ExecutorControl(InterruptYielding)` | runtime adapter control path in `meerkat_machine.rs`; no separate current session-layer lowering is discovered |
| `ExecutorControl(StopRuntimeExecutor)` | `MeerkatMachine::stop_runtime_executor(...)` and attached control queue |
| `RetireRuntime` | `MeerkatMachine::retire_runtime(...)` / trait lowering in `meerkat_machine.rs` |
| `ResetRuntime` | `MeerkatMachine::reset_runtime(...)` / trait lowering in `meerkat_machine.rs` |
| `RecoverRuntime` | `MeerkatMachine::recover(...)` |
| `RecycleRuntime` | `MeerkatMachine::recycle(...)` |
| `DestroyRuntime` | `MeerkatMachine::destroy(...)` |

## Admission / ingress lowering

| Machine input/effect | Current lowering |
| --- | --- |
| `AdmitQueuedInput` | runtime-backed session start-turn / request execution path; visible in Meerkat spine tests as `inputs.queue` |
| `AdmitSteeredInput` | runtime-backed session start-turn / resume path; visible in spine tests as `inputs.steer_queue` with `wake_requested` and `process_requested` |
| `WakeRuntimeLoop` | `wake_tx.try_send(())` / `wake_tx.send(())` in `meerkat_machine.rs` |
| `UpdatePeerIngressContext` | `MeerkatMachine::update_peer_ingress_context(...)` |
| `EnsureDrainRunning` | `MeerkatMachine::maybe_spawn_comms_drain(...)` |

## Turn / ops / barrier lowering

| Machine input/effect | Current lowering |
| --- | --- |
| `RegisterPendingOps` | `TurnExecutionAuthority::apply(TurnExecutionInput::RegisterPendingOps { .. })` inside the runner loop |
| `WaitAllRequested` | `RuntimeOpsLifecycleRegistry::wait_all(&run_id, &barrier_ids)` in `meerkat-core/src/agent/state.rs` |
| `WaitAllSatisfied` / `OpsBarrierSatisfied` | generated `protocol_ops_barrier_satisfaction::{accept_wait_all_satisfied, submit_ops_barrier_satisfied}` |
| `ToolCallsResolved` | `TurnExecutionInput::ToolCallsResolved { .. }` in the runner loop |
| `BoundaryContinue` | `TurnExecutionInput::BoundaryContinue { .. }` in the runner loop |
| `BoundaryComplete` | `TurnExecutionInput::BoundaryComplete { .. }` in the runner loop |
| `CancellationObserved` | `TurnExecutionInput::CancellationObserved { .. }` in the runner loop |
| `RetryRequested` | `TurnExecutionInput::RetryRequested { .. }` in the runner loop |
| `BoundaryApplied` / `CheckCompaction` / `RunCancelled` | effects emitted from `TurnExecutionAuthority` transitions and executed in the runner path |

## Peer-ingress lowering

| Machine input/effect | Current lowering |
| --- | --- |
| `RegisterTrustedPeer` / `UnregisterTrustedPeer` / `UnregisterTrustedPubkey` | canonical trust mutation seams in `meerkat-comms/src/runtime/comms_runtime.rs` |
| `ReceivePeerIngress` | classified inbox + `PeerCommsAuthority` receive path in `meerkat-comms` |
| `DrainPeerIngress` | `AgentToolDispatcher::drain_peer_input_candidates(...)` forwarding into classified inbox drain |
| `peer_ingress_runtime_snapshot` observation | `CommsRuntime::peer_ingress_runtime_snapshot()` -> core trait -> Meerkat join |

## External tool-surface lowering

| Machine input/effect | Current lowering |
| --- | --- |
| `StageAdd` / `StageRemove` / `StageReload` | `McpRouter::{stage_add, stage_remove, stage_reload}` |
| `ApplyBoundary` | `McpRouter::apply_staged()` |
| `PendingSucceeded` | generated `protocol_surface_completion::submit_pending_succeeded(...)` in router completion paths |
| `FinalizeRemovalClean` / `FinalizeRemovalForced` | router removal finalization path in `process_removals(...)` |
| `PublishToolSurfaceSnapshot` | router snapshot publication -> runner -> session -> service-factory forwarding |

## Drain / keep-alive lowering

| Machine input/effect | Current lowering |
| --- | --- |
| `EnsureDrainRunning` | `protocol_comms_drain_spawn::execute_ensure_running(...)` inside `maybe_spawn_comms_drain(...)` |
| `TaskSpawned` | `protocol_comms_drain_spawn::submit_task_spawned(...)` inside `maybe_spawn_comms_drain(...)` |
| `TaskExited` | `MeerkatMachine::notify_comms_drain_exited(...)` |
| `AbortDrainTask` | `MeerkatMachine::abort_comms_drain(...)` / `abort_comms_drains(...)` |

## Explicit non-lowerings

The following are present in lower-level authority/model assets but are **not**
discovered live top-level lowerings today:

- `CancelAfterBoundary`
- public non-`PersistentHost` drain-mode lowerings
- a store-backed peer-ingress recovery replay
- a store-backed Meerkat tool-surface recovery replay

Those exclusions are part of the exact current freeze, not omissions.

## Freeze decision

`K8` is considered frozen for the exact current Meerkat boundary.

This lowering map is sufficient to start exact-current TLA+ work without
pretending the current implementation is already a write-side kernel switch.
