# MeerkatMachine Coverage Matrix

Status: frozen target-state coverage handoff

This note proves that the target `MeerkatMachine` freeze package covers:

- every target input
- every target effect
- every machine region

Use it with:

- `meerkat-machine-freeze.md`
- `meerkat-machine-transition-catalog.md`
- `meerkat-machine-proof-obligations.md`

## Purpose

A target freeze is still incomplete if the alphabet says one thing and the
transition catalog silently forgets part of it.

This matrix exists to make that impossible.

## Input Coverage

| Input | Transition home | Primary regions |
| --- | --- | --- |
| `PrepareBindings` | Binding lifecycle | `binding`, `control` |
| `RegisterSession` | Binding lifecycle | `binding`, `control`, `drain` |
| `RegisterSessionWithExecutor` | Binding lifecycle | `binding`, `control`, `drain` |
| `InterruptCurrentRun` | Run start and control | `control`, `turn` |
| `InterruptYielding` | Run start and control | `control`, `turn` |
| `CancelAfterBoundary` | Run start and control | `turn`, `control` |
| `StopRuntime` | Recovery / recycle / reset / retire / stop / destroy | `control`, `inputs`, `completion`, `drain` |
| `RetireRuntime` | Recovery / recycle / reset / retire / stop / destroy | `control`, `inputs`, `completion`, `turn` |
| `ResetRuntime` | Recovery / recycle / reset / retire / stop / destroy | `binding`, `control`, `inputs`, `completion`, `peer`, `tools` |
| `RecoverRuntime` | Recovery / recycle / reset / retire / stop / destroy | all regions |
| `RecycleRuntime` | Recovery / recycle / reset / retire / stop / destroy | `binding`, `inputs`, `completion`, `ops`, `peer`, `tools`, `drain` |
| `DestroyRuntime` | Recovery / recycle / reset / retire / stop / destroy | all regions |
| `AdmitQueuedInput` | Admission | `inputs`, `completion`, `control` |
| `AdmitSteeredInput` | Admission | `inputs`, `completion`, `control` |
| `AdmitPeerIngress` | Admission | `peer`, `inputs`, `completion`, `control` |
| `ReplayPeerIngress` | Admission | `peer`, `inputs`, `completion`, `control` |
| `UpdatePeerIngressContext` | Peer region | `peer`, `drain` |
| `RegisterPendingOps` | Turn / ops / barrier | `turn`, `ops` |
| `WaitAllRequested` | Turn / ops / barrier | `ops`, `turn` |
| `WaitAllSatisfied` | Turn / ops / barrier | `ops`, `turn` |
| `OpsBarrierSatisfied` | Turn / ops / barrier | `turn`, `ops` |
| `ToolCallsResolved` | Turn / ops / barrier | `turn`, `ops` |
| `BoundaryContinue` | Turn / ops / barrier | `turn`, `inputs`, `tools` |
| `BoundaryComplete` | Turn / ops / barrier | `turn`, `inputs`, `tools` |
| `CancellationObserved` | Turn / ops / barrier | `turn`, `control` |
| `RetryRequested` | Turn / ops / barrier | `turn` |
| `RegisterTrustedPeer` | Peer region | `peer` |
| `UnregisterTrustedPeer` | Peer region | `peer` |
| `ReceivePeerIngress` | Peer region | `peer` |
| `DrainPeerIngress` | Peer region | `peer`, `inputs`, `completion`, `control` |
| `StageAdd` | Tool surface | `tools` |
| `StageRemove` | Tool surface | `tools` |
| `StageReload` | Tool surface | `tools` |
| `ApplyBoundary` | Tool surface | `tools`, `turn` |
| `PendingSucceeded` | Turn / ops / barrier | `ops`, `turn` |
| `PendingFailed` | Turn / ops / barrier | `ops`, `turn` |
| `FinalizeRemovalClean` | Tool surface | `tools` |
| `FinalizeRemovalForced` | Tool surface | `tools` |
| `EnsureDrainRunning` | Drain lifecycle | `drain`, `binding` |
| `TaskSpawned` | Drain lifecycle | `drain` |
| `TaskExited` | Drain lifecycle | `drain`, `binding`, `control` |
| `AbortDrain` | Drain lifecycle | `drain`, `control` |

## Effect Coverage

| Effect | Source transition home |
| --- | --- |
| `WakeRuntimeLoop` | `AdmitQueuedInput`, `AdmitSteeredInput`, `AdmitPeerIngress`, `ReplayPeerIngress` |
| `ExecutorControl(CancelCurrentRun)` | `InterruptCurrentRun` |
| `ExecutorControl(InterruptYielding)` | `InterruptYielding` |
| `ExecutorControl(StopRuntimeExecutor)` | `StopRuntime` |
| `ResolveCompletionWaiters` | `ResolveCompletionWaiters` |
| `WaitAllRequested` | `WaitAllRequested` |
| `InjectDetachedWakeContinuation` | `InjectDetachedWakeContinuation` |
| `PublishToolSurfaceSnapshot` | `ApplyBoundary` |
| `EmitExternalToolUpdate` | `ApplyBoundary` |
| `SpawnDrainTask` | `EnsureDrainRunning` |
| `AbortDrainTask` | `AbortDrain` |
| `PersistRecoverySnapshot` | `PrepareBindings`, lifecycle transitions that change reconstructable truth, and recovery/recycle completion |

## Region Coverage

| Region | Owning transition homes |
| --- | --- |
| `binding` | Binding lifecycle; recovery / recycle / reset / retire / stop / destroy |
| `control` | Binding lifecycle; admission; run start and control; turn / ops / barrier; drain lifecycle |
| `inputs` | Admission; turn / ops / barrier; peer region; recovery / recycle / reset / retire / stop / destroy |
| `completion` | Admission; completion; peer region; recovery / recycle / reset / retire / stop / destroy |
| `turn` | Run start and control; turn / ops / barrier; recovery / recycle / reset / retire / stop / destroy |
| `ops` | Turn / ops / barrier; detached wake; recovery / recycle / reset / retire / stop / destroy |
| `peer` | Peer region; admission; recovery / recycle / reset |
| `tools` | Tool surface; turn / ops / barrier; recovery / recycle / reset |
| `drain` | Drain lifecycle; binding lifecycle; recovery / recycle / reset / stop / destroy |

## Acceptance

This matrix is complete enough when:

- every target input has one transition home
- every target effect has one or more source homes
- every machine region has explicit owning transition homes

That state is now reached.
