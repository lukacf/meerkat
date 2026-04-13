# Meerkat Cutover Lowering Inventory

Status: active pre-cutover inventory

This note turns `meerkat-lowering-map.md` and
`meerkat-machine-refinement-delta.md` into a cutover-facing execution list.

It answers one concrete question:

- which `MeerkatMachine` inputs/effects are already lowered through one
  canonical seam, and which still need centralization before write-side cutover

## Classification

Each entry is classified as one of:

- `canonical_ready`: one named implementation seam is already a plausible
  write-side cutover path
- `needs_centralization`: the semantics are frozen, but the implementation
  still reaches them through too many helper-rich paths
- `target_only`: the target machine owns the concept, but no honest live
  top-level lowering exists yet

## Control / lifecycle

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `PrepareBindings` | `MeerkatMachine::prepare_bindings(...)` | `canonical_ready` |
| `RegisterSession` | `MeerkatMachine::register_session(...)` | `canonical_ready` |
| `RegisterSessionWithExecutor` | `MeerkatMachine::register_session_with_executor(...)` | `canonical_ready` |
| `InterruptCurrentRun` | `MeerkatMachine::interrupt_current_run(...)` | `canonical_ready` |
| `ExecutorControl(CancelCurrentRun)` | attached runtime control queue in `meerkat_machine.rs` | `needs_centralization` |
| `ExecutorControl(InterruptYielding)` | runtime adapter control path in `meerkat_machine.rs` | `needs_centralization` |
| `ExecutorControl(StopRuntimeExecutor)` | `MeerkatMachine::stop_runtime_executor(...)` plus attached queue | `needs_centralization` |
| `RetireRuntime` | `MeerkatMachine::retire_runtime(...)` and trait lowering | `needs_centralization` |
| `ResetRuntime` | `MeerkatMachine::reset_runtime(...)` | `needs_centralization` |
| `RecoverRuntime` | `MeerkatMachine::recover(...)` | `needs_centralization` |
| `RecycleRuntime` | `MeerkatMachine::recycle(...)` | `needs_centralization` |
| `DestroyRuntime` | `MeerkatMachine::destroy(...)` | `needs_centralization` |
| `CancelAfterBoundary` | `MeerkatMachine::cancel_after_boundary(...)` -> session-service boundary-cancel flag | `canonical_ready` |

## Admission / input ledger

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `AdmitQueuedInput` | session request/start-turn path | `needs_centralization` |
| `AdmitSteeredInput` | session resume/start-turn path | `needs_centralization` |
| `WakeRuntimeLoop` | adapter wake channel send | `canonical_ready` |
| `PersistRecoverySnapshot` | recovery/build helper stack rather than one machine seam | `needs_centralization` |

## Turn / ops / barrier

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `RegisterPendingOps` | `TurnExecutionAuthority::apply(...)` in runner loop | `needs_centralization` |
| `WaitAllRequested` | `RuntimeOpsLifecycleRegistry::wait_all(...)` | `needs_centralization` |
| `WaitAllSatisfied` | protocol barrier-satisfaction submission path | `needs_centralization` |
| `OpsBarrierSatisfied` | protocol barrier-satisfaction submission path | `needs_centralization` |
| `ToolCallsResolved` | `TurnExecutionInput::ToolCallsResolved` | `needs_centralization` |
| `BoundaryContinue` | `TurnExecutionInput::BoundaryContinue` | `needs_centralization` |
| `BoundaryComplete` | `TurnExecutionInput::BoundaryComplete` | `needs_centralization` |
| `CancellationObserved` | `TurnExecutionInput::CancellationObserved` | `needs_centralization` |
| `RetryRequested` | `TurnExecutionInput::RetryRequested` | `needs_centralization` |
| `BoundaryApplied` / `CheckCompaction` / `RunCancelled` | emitted effects executed in runner path | `needs_centralization` |

## Peer ingress

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `RegisterTrustedPeer` / `UnregisterTrustedPeer` / `UnregisterTrustedPubkey` | canonical comms runtime trust seam | `canonical_ready` |
| `ReceivePeerIngress` | classified inbox + `PeerCommsAuthority` receive path | `needs_centralization` |
| `DrainPeerIngress` | dispatcher drain forwarding into classified inbox drain | `needs_centralization` |
| `UpdatePeerIngressContext` | `MeerkatMachine::update_peer_ingress_context(...)` | `canonical_ready` |

## Tools

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `SetToolVisibilityState` | `SessionCommand::SetToolVisibilityState` via `EphemeralSessionService` | `canonical_ready` |
| `PublishToolVisibilityProjection` | `ToolScope` projection over durable visibility state | `needs_centralization` |
| `StageAdd` / `StageRemove` / `StageReload` | `McpRouter::{stage_add, stage_remove, stage_reload}` | `canonical_ready` |
| `ApplyBoundary` | `McpRouter::apply_staged()` | `canonical_ready` |
| `PendingSucceeded` | router completion protocol path | `needs_centralization` |
| `FinalizeRemovalClean` / `FinalizeRemovalForced` | router removal finalization path | `needs_centralization` |
| `PublishToolSurfaceSnapshot` | router -> runner -> session -> factory forwarding | `needs_centralization` |
| `PublishCommittedVisibleSet` | `Agent::publish_committed_visible_set()` during boundary visibility apply | `canonical_ready` |

## Drain / keep-alive

| Input/effect | Current lowering | Status |
| --- | --- | --- |
| `EnsureDrainRunning` | protocol spawn inside `maybe_spawn_comms_drain(...)` | `needs_centralization` |
| `TaskSpawned` | protocol spawned submission inside `maybe_spawn_comms_drain(...)` | `needs_centralization` |
| `TaskExited` | `MeerkatMachine::notify_comms_drain_exited(...)` | `needs_centralization` |
| `AbortDrainTask` | adapter abort helpers | `needs_centralization` |

## Immediate cutover priorities

Prioritize these first:

1. centralize lifecycle commands on one adapter-owned machine seam
2. centralize turn / ops / barrier progression away from helper-rich runner
   pathways
3. finish the tools publication contract so visibility + surface become one
   committed machine-facing story
4. make peer ingress drain / receive lower through one explicit machine seam
5. collapse drain lifecycle onto one canonical authority path

## Read with

- `meerkat-machine-refinement-delta.md`
- `meerkat-lowering-map.md`
- `meerkat-tool-visibility-freeze.md`
- `meerkat-tool-surface-freeze.md`
