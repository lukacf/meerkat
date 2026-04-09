# Meerkat Input / Effect Alphabet

Status: frozen exact-current-state asset

This note closes `K7` from `meerkat-cutover-checklist.md`.

It defines the exact current machine input/effect alphabet for
`MeerkatMachine`, at the granularity needed for exact-current TLA+ modeling.

This is **not** a claim that every generated authority verb already has a live
surface lowering. It is the current machine alphabet with explicit
classifications for:

- live public/runtime inputs
- live internal owner inputs
- authority-local or target-state verbs that are not part of the current
  top-level Meerkat boundary

## Machine inputs

### Binding / control inputs

| Input | Exact current meaning |
| --- | --- |
| `PrepareBindings` | Create or return the hidden runtime binding for a session. |
| `RegisterSession` | Register a runtime-backed session without an attached executor loop. |
| `RegisterSessionWithExecutor` | Register a session and attach a live executor/control loop. |
| `InterruptCurrentRun` | Deferred hard-cancel request on attached runtimes; `NotReady` without attached loop. |
| `InterruptYielding` | Deferred cooperative interrupt request at the runtime-adapter seam. |
| `StopRuntimeExecutor` | Stop / detach control seam; on attached paths it may defer until `apply()` finishes. |
| `RetireRuntime` | Retire runtime state, with exact-current split-lifetime behavior for queued work vs `wait_all`. |
| `ResetRuntime` | Reset runtime state, clearing input-owned queued work/waiters and preserving or terminating `wait_all` according to the current path. |
| `RecoverRuntime` | Recover runtime state from the current live binding/driver truth. |
| `RecycleRuntime` | Recycle preserving queued work on the exact current paths that support it. |
| `DestroyRuntime` | Destroy runtime state immediately, with exact-current split-lifetime behavior for input waiters vs `wait_all`. |

### Admission / ingress inputs

| Input | Exact current meaning |
| --- | --- |
| `AdmitQueuedInput` | Ordinary queued admission into `inputs.queue`. |
| `AdmitSteeredInput` | Steered admission into `inputs.steer_queue` with `wake_requested` and `process_requested`. |
| `AdmitPeerIngress` | Classified peer ingress admission through the comms-runtime / peer-authority seam. |
| `UpdatePeerIngressContext` | Public surface-facing keep-alive + comms-runtime context update seam. |

### Turn / ops / barrier inputs

| Input | Exact current meaning |
| --- | --- |
| `RegisterPendingOps` | Record the pending-op set and barrier-op subset for `WaitingForOps`. |
| `OpsBarrierSatisfied` | Authority-owned handoff from `wait_all` barrier completion back into turn execution. |
| `ToolCallsResolved` | Allowed only after barrier satisfaction; advances to boundary draining. |
| `BoundaryContinue` | Continue a draining boundary back into `CallingLlm` or `Cancelled`, depending on current exact authority state. |
| `BoundaryComplete` | Finish a draining boundary into a terminal turn state. |
| `CancellationObserved` | Current hard-cancel observation seam in the turn authority. |
| `RetryRequested` | Current turn recovery / retry seam. |

### Peer-ingress inputs

| Input | Exact current meaning |
| --- | --- |
| `RegisterTrustedPeer` | Canonical runtime trust-registration seam. |
| `UnregisterTrustedPeer` / `UnregisterTrustedPubkey` | Canonical runtime trust-removal seam. |
| `ReceivePeerIngress` | Live classified raw peer input entering the peer authority / queue seam. |
| `DrainPeerIngress` | Runtime drain bridge from classified queue to agent-facing peer input candidates. |

### External tool-surface inputs

| Input | Exact current meaning |
| --- | --- |
| `StageAdd` | Record add intent in the tool-surface authority. |
| `StageRemove` | Record remove intent in the tool-surface authority. |
| `StageReload` | Record reload intent in the tool-surface authority. |
| `ApplyBoundary` | Apply staged tool-surface intent at the current boundary. |
| `PendingSucceeded` | Completion feedback for non-blocking add/reload paths. |
| `FinalizeRemovalClean` / `FinalizeRemovalForced` | Finalize removal lifecycle through the authority. |

### Drain / keep-alive inputs

| Input | Exact current meaning |
| --- | --- |
| `EnsureDrainRunning` | Canonical drain-lifecycle request behind `update_peer_ingress_context(...)`. |
| `TaskSpawned` | Drain task spawn obligation closed synchronously by the adapter. |
| `TaskExited` | Drain task exit notification back into authority state. |
| `AbortDrain` | Explicit drain abort through adapter lifecycle control. |

## Machine effects

| Effect | Exact current meaning |
| --- | --- |
| `WakeRuntimeLoop` | Signal the runtime loop that queued or steered work is ready. |
| `ExecutorControl(CancelCurrentRun)` | Deferred hard-cancel control command to attached runtime loop. |
| `ExecutorControl(InterruptYielding)` | Deferred cooperative interrupt control command to attached runtime loop. |
| `ExecutorControl(StopRuntimeExecutor)` | Deferred stop control command to attached runtime loop. |
| `ResolveCompletionWaiters` | Complete or terminate input-owned completion waiters. |
| `WaitAllRequested` | Ops-lifecycle barrier wait request. |
| `WaitAllSatisfied` | Ops-lifecycle completion handoff back into turn execution. |
| `BoundaryApplied` | Turn boundary publication. |
| `CheckCompaction` | Current continuation effect emitted after `BoundaryContinue`. |
| `RunCancelled` | Current terminal cancellation effect emitted by turn authority. |
| `InjectDetachedWakeContinuation` | Exact-current detached background completion continuation injection. |
| `SpawnDrainTask` | Start the comms drain task. |
| `AbortDrainTask` | Abort the comms drain task. |
| `PublishToolSurfaceSnapshot` | Publish current external tool-surface projection. |
| `EmitExternalToolUpdate` | External tool lifecycle update emission. |

## Explicit exclusions

The following are present in lower-level authority/model assets but are
explicitly **not** part of the exact current top-level Meerkat boundary:

- `cancel_after_boundary` as a live lowered runtime/session action
- public lowering for non-`PersistentHost` drain modes
- durable replay of peer-ingress queue contents
- a second store-backed tool-surface recovery protocol above the live router
  authority

## Freeze decision

`K7` is considered frozen for the exact current Meerkat boundary.

This alphabet is the basis for exact-current TLA+ work on `MeerkatMachine`.
