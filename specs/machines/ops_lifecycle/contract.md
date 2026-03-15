# OpsLifecycleMachine

Status: normative `0.5` machine contract

## Purpose

`OpsLifecycleMachine` owns the shared async-operation lifecycle substrate for a
single runtime/session instance.

It is the authoritative owner of:

- operation registration/provisioning state
- running vs terminal async-operation status
- progress reporting
- terminal outcome buffering
- terminal watcher/completion resolution
- typed lifecycle event emission across the runtime boundary

It is **not** the owner of:

- parent/child conversational peer traffic
- runtime work admission itself
- mob topology/flow policy
- transcript or UI projection of completion
- tool-local process or transport mechanics

`0.5` does **not** keep a separate lightweight subagent mechanism.

There are only two user-facing agent wiring levels:

- `Comms`
- `Mobs`

Child-agent behavior therefore lives on the mob path, while background async
tool work shares the same lifecycle substrate without becoming a third wiring
level.

## Scope Boundary

This machine begins after a caller has decided to create or manage async work
and has handed lifecycle ownership into the ops substrate.

It ends at:

- typed `OpEvent`
- watcher resolution
- retained lifecycle state for inspection

Conversation with a mob-backed child remains in `PeerCommsMachine`.

## Authoritative State Model

For one owner/runtime instance, the machine state is the tuple:

- `operation_status: Map<OperationId, OperationStatus>`
- `operation_kind: Map<OperationId, OperationKind>`
- `peer_ready: Map<OperationId, Bool>`
- `progress_count: Map<OperationId, u32>`
- `watcher_count: Map<OperationId, u32>`
- `terminal_outcome: Map<OperationId, OperationTerminalOutcome>`
- `terminal_buffered: Map<OperationId, Bool>`

`OperationStatus` is the closed state set:

- `Absent`
- `Provisioning`
- `Running`
- `Retiring`
- `Completed`
- `Failed`
- `Cancelled`
- `Retired`
- `Terminated`

Terminal states:

- `Completed`
- `Failed`
- `Cancelled`
- `Retired`
- `Terminated`

`OperationKind` is:

- `None`
- `MobMemberChild`
- `BackgroundToolOp`

Shell jobs are a specialization of `BackgroundToolOp` in shell/runtime code,
not a separate machine kind.

`OperationTerminalOutcome` is:

- `None`
- `Completed`
- `Failed`
- `Cancelled`
- `Retired`
- `Terminated`

## Input Alphabet

The closed external input/command alphabet for this machine is:

- `RegisterOperation(operation_id, operation_kind)`
- `ProvisioningSucceeded(operation_id)`
- `ProvisioningFailed(operation_id)`
- `PeerReady(operation_id)`
- `RegisterWatcher(operation_id)`
- `ProgressReported(operation_id)`
- `CompleteOperation(operation_id)`
- `FailOperation(operation_id)`
- `CancelOperation(operation_id)`
- `RetireRequested(operation_id)`
- `RetireCompleted(operation_id)`
- `CollectTerminal(operation_id)`
- `OwnerTerminated`

Notes:

- child-agent operations are mob-backed and use `MobMemberChild`
- background async tool work uses `BackgroundToolOp`
- `PeerReady` only applies to operation kinds that actually participate in the
  peer plane

## Effect Family

The closed machine-boundary effect family is:

- `SubmitOpEvent(operation_id, event_kind)`
- `NotifyOpWatcher(operation_id, terminal_outcome)`
- `ExposeOperationPeer(operation_id)`
- `RetainTerminalRecord(operation_id, terminal_outcome)`

Architecture rule:

- transcript/tool-return/UI delivery is a projection of these lifecycle effects
  and typed `OperationInput`, not the authoritative lifecycle contract itself

## Transition Relation

### Registration and provisioning

1. `RegisterOperation`

Preconditions:

- `operation_status[operation_id] = Absent`
- `operation_kind != None`

State updates:

- `operation_status[operation_id] := Provisioning`
- `operation_kind[operation_id] := operation_kind`
- `peer_ready[operation_id] := FALSE`
- `progress_count[operation_id] := 0`
- `watcher_count[operation_id] := 0`
- `terminal_outcome[operation_id] := None`
- `terminal_buffered[operation_id] := FALSE`

2. `ProvisioningSucceeded`

Preconditions:

- `operation_status[operation_id] = Provisioning`

State updates:

- `operation_status[operation_id] := Running`

Effect:

- `SubmitOpEvent(operation_id, Started)`

3. `ProvisioningFailed`

Preconditions:

- `operation_status[operation_id] = Provisioning`

State updates:

- `operation_status[operation_id] := Failed`
- `terminal_outcome[operation_id] := Failed`
- `terminal_buffered[operation_id] := TRUE`

Effects:

- `SubmitOpEvent(operation_id, Failed)`
- `NotifyOpWatcher(operation_id, Failed)`
- `RetainTerminalRecord(operation_id, Failed)`

### Nonterminal execution

4. `PeerReady`

Preconditions:

- `operation_status[operation_id] ∈ {Running, Retiring}`
- `operation_kind[operation_id] = MobMemberChild`

State updates:

- `peer_ready[operation_id] := TRUE`

Effect:

- `ExposeOperationPeer(operation_id)`

5. `RegisterWatcher`

Preconditions:

- `operation_status[operation_id] != Absent`

State updates:

- `watcher_count[operation_id] += 1`

6. `ProgressReported`

Preconditions:

- `operation_status[operation_id] ∈ {Running, Retiring}`

State updates:

- `progress_count[operation_id] += 1`

Effect:

- `SubmitOpEvent(operation_id, Progress)`

### Terminalization

7. `CompleteOperation`

Preconditions:

- `operation_status[operation_id] ∈ {Running, Retiring}`

State updates:

- `operation_status[operation_id] := Completed`
- `terminal_outcome[operation_id] := Completed`
- `terminal_buffered[operation_id] := TRUE`

Effects:

- `SubmitOpEvent(operation_id, Completed)`
- `NotifyOpWatcher(operation_id, Completed)`
- `RetainTerminalRecord(operation_id, Completed)`

8. `FailOperation`

Preconditions:

- `operation_status[operation_id] ∈ {Provisioning, Running, Retiring}`

State updates:

- `operation_status[operation_id] := Failed`
- `terminal_outcome[operation_id] := Failed`
- `terminal_buffered[operation_id] := TRUE`

Effects:

- `SubmitOpEvent(operation_id, Failed)`
- `NotifyOpWatcher(operation_id, Failed)`
- `RetainTerminalRecord(operation_id, Failed)`

9. `CancelOperation`

Preconditions:

- `operation_status[operation_id] ∈ {Provisioning, Running, Retiring}`

State updates:

- `operation_status[operation_id] := Cancelled`
- `terminal_outcome[operation_id] := Cancelled`
- `terminal_buffered[operation_id] := TRUE`

Effects:

- `SubmitOpEvent(operation_id, Cancelled)`
- `NotifyOpWatcher(operation_id, Cancelled)`
- `RetainTerminalRecord(operation_id, Cancelled)`

### Retire path

10. `RetireRequested`

Preconditions:

- `operation_status[operation_id] = Running`

State updates:

- `operation_status[operation_id] := Retiring`

11. `RetireCompleted`

Preconditions:

- `operation_status[operation_id] = Retiring`

State updates:

- `operation_status[operation_id] := Retired`
- `terminal_outcome[operation_id] := Retired`
- `terminal_buffered[operation_id] := TRUE`

Effects:

- `SubmitOpEvent(operation_id, Retired)`
- `NotifyOpWatcher(operation_id, Retired)`
- `RetainTerminalRecord(operation_id, Retired)`

### Buffer drain and shutdown

12. `CollectTerminal`

Preconditions:

- `operation_status[operation_id] ∈ {Completed, Failed, Cancelled, Retired, Terminated}`
- `terminal_buffered[operation_id] = TRUE`

State updates:

- `terminal_buffered[operation_id] := FALSE`

13. `OwnerTerminated`

State updates:

- every operation in `Provisioning`, `Running`, or `Retiring` becomes
  `Terminated`
- each such operation gets:
  - `terminal_outcome := Terminated`
  - `terminal_buffered := TRUE`

Effects:

- `SubmitOpEvent(operation_id, Terminated)` for each affected operation
- `NotifyOpWatcher(operation_id, Terminated)` for each affected operation

## Invariants

The machine must preserve:

1. only one terminal outcome per operation
2. terminal states never transition back to nonterminal states
3. `terminal_buffered = TRUE` only for terminal operation states
4. `peer_ready = TRUE` implies `operation_kind = MobMemberChild`
5. `peer_ready = TRUE` implies the operation is not `Absent`
6. non-`Absent` operation IDs keep their kind identity
7. watcher registration does not itself change lifecycle state

## Implementation Anchors

Current `0.4` anchors include:

- `meerkat-core/src/ops.rs`
- `meerkat-core/src/sub_agent.rs`
- `meerkat-core/src/agent/runner.rs`
- `meerkat-mob/src/runtime/actor.rs`
- `meerkat-mob/src/runtime/provisioner.rs`
- `meerkat-mob/src/roster.rs`
- `meerkat-tools/src/builtin/shell/job_manager.rs`

`0.5` turns these into one explicit lifecycle substrate rather than leaving
mob-backed child work and background async operations as separate authoritative
owners.
