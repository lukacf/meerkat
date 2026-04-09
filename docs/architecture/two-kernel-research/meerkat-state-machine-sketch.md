# Meerkat Internal State Machine Sketch

Status: supporting state sketch (target decisions frozen in `meerkat-machine-freeze.md`)

## Purpose

This document turns the Meerkat kernel shape into an explicit internal machine
sketch.

It is not yet a formal contract. It is a design scaffold that answers:

- what the top-level Meerkat state variables are
- how those variables cluster into internal regions
- which transition families update which regions
- where the hardest cross-region couplings live

The intent is to make `MeerkatMachine` concrete enough that later TLA or Rust
refactoring work has a single internal target shape.

## Design Stance

`MeerkatMachine` is one kernel with grouped internal state variables.

It is not:

- one giant flat bag of fields with no structure
- eight independent semantic authorities loosely coordinated by routes

The working structure is:

```text
MeerkatMachine =
  binding
  + control
  + inputs
  + turn
  + ops
  + peer
  + tools
  + drain
```

Each group is internally authoritative for its facts, but the combined machine
owns the cross-group transitions.

## Top-Level Variable Groups

### `binding`

```text
binding = {
  session_id: SessionId,
  runtime_id: LogicalRuntimeId,
  driver_present: Bool,
  attachment_live: Bool,
  completions_present: Bool,
  ops_registry_present: Bool,
  epoch_id: RuntimeEpochId,
  cursor_state: {
    agent_applied_cursor: u64,
    runtime_observed_seq: u64,
    runtime_last_injected_seq: u64
  }
}
```

This is the hidden runtime-binding state that makes the rest of the kernel
real. `epoch_id` and `cursor_state` are deliberately internal.

### `control`

```text
control = {
  phase: Initializing | Idle | Attached | Running | Recovering | Retired | Stopped | Destroyed,
  current_run_id: Option<RunId>,
  pre_run_phase: Option<RuntimePhase>,
  wake_pending: Bool,
  process_pending: Bool
}
```

This group owns lifecycle and out-of-band control precedence.

### `inputs`

```text
inputs = {
  admitted: Set<WorkId>,
  admission_order: Seq<WorkId>,
  content_shape: Map<WorkId, ContentShape>,
  request_id: Map<WorkId, Option<RequestId>>,
  reservation_key: Map<WorkId, Option<ReservationKey>>,
  policy_snapshot: Map<WorkId, PolicyDecision>,
  handling_mode: Map<WorkId, HandlingMode>,
  lifecycle: Map<WorkId, InputLifecycleState>,
  terminal_outcome: Map<WorkId, Option<InputTerminalOutcome>>,
  queue: Seq<WorkId>,
  steer_queue: Seq<WorkId>,
  current_run_id: Option<RunId>,
  current_run_contributors: Seq<WorkId>,
  last_run_id: Map<WorkId, Option<RunId>>,
  last_boundary_sequence: Map<WorkId, Option<BoundarySequence>>,
  silent_intent_overrides: Set<String>
}
```

This group is the admitted-work truth. Queue order is a projection inside the
same region, not a separate authority.

### `turn`

```text
turn = {
  phase: Ready | ApplyingPrimitive | CallingLlm | WaitingForOps | DrainingBoundary |
         Extracting | ErrorRecovery | Cancelling | Completed | Failed | Cancelled,
  active_run_id: Option<RunId>,
  primitive_kind: TurnPrimitiveKind,
  admitted_content_shape: Option<ContentShape>,
  vision_enabled: Bool,
  image_tool_results_enabled: Bool,
  tool_calls_pending: u32,
  pending_op_refs: Option<Seq<AsyncOpRef>>,
  barrier_operation_ids: Seq<OperationId>,
  has_barrier_ops: Bool,
  barrier_satisfied: Bool,
  boundary_count: u32,
  cancel_after_boundary: Bool,
  terminal_outcome: TurnTerminalOutcome,
  extraction_attempts: u32,
  max_extraction_retries: u32
}
```

This is the execution heart. It stays semantically narrow, but it is deeply
coupled to `inputs`, `ops`, and `tools`.

### `ops`

```text
ops = {
  known_operations: Set<OperationId>,
  operation_status: Map<OperationId, OperationStatus>,
  operation_kind: Map<OperationId, OperationKind>,
  peer_ready: Map<OperationId, Bool>,
  progress_count: Map<OperationId, u32>,
  watcher_count: Map<OperationId, u32>,
  terminal_outcome: Map<OperationId, OperationTerminalOutcome>,
  terminal_buffered: Map<OperationId, Bool>,
  completed_order: Seq<OperationId>,
  max_completed: u32,
  max_concurrent: u32,
  active_count: u32,
  created_at_ms: Map<OperationId, u64>,
  completed_at_ms: Map<OperationId, u64>,
  wait_active: Bool,
  wait_request_id: WaitRequestId,
  wait_operation_ids: Seq<OperationId>
}
```

This group owns every async operation and all wait/barrier truth derived from
them.

### `peer`

```text
peer = {
  phase: Absent | Received | Dropped | Delivered,
  self_peer_id: PeerId,
  auth_required: Bool,
  trusted_peers: Set<PeerId>,
  raw_item_peer: Map<RawItemId, PeerId>,
  raw_item_kind: Map<RawItemId, RawPeerKind>,
  classified_as: Map<RawItemId, PeerInputClass>,
  text_projection: Map<RawItemId, String>,
  content_shape: Map<RawItemId, ContentShape>,
  request_id: Map<RawItemId, Option<RequestId>>,
  reservation_key: Map<RawItemId, Option<ReservationKey>>,
  lifecycle_peer: Map<RawItemId, Option<String>>,
  trusted_snapshot: Map<RawItemId, Bool>,
  submission_queue: Seq<RawItemId>
}
```

This group keeps peer normalization inside Meerkat and lowers valid peer work
into ordinary admission. `RawItemId` is the stable ingress identity for queued
peer work; snapshots may project it directly, but the semantic owner remains
the peer region itself.

### `tools`

```text
tools = {
  phase: Operating | Shutdown,
  known_surfaces: Set<SurfaceId>,
  visible_surfaces: Set<SurfaceId>,
  base_state: Map<SurfaceId, SurfaceBaseState>,
  pending_op: Map<SurfaceId, PendingSurfaceOp>,
  staged_op: Map<SurfaceId, StagedSurfaceOp>,
  staged_intent_sequence: Map<SurfaceId, u64>,
  next_staged_intent_sequence: u64,
  pending_task_sequence: Map<SurfaceId, u64>,
  pending_lineage_sequence: Map<SurfaceId, u64>,
  next_pending_task_sequence: u64,
  inflight_calls: Map<SurfaceId, u64>,
  last_delta_operation: Map<SurfaceId, SurfaceDeltaOperation>,
  last_delta_phase: Map<SurfaceId, SurfaceDeltaPhase>,
  snapshot_epoch: u64,
  snapshot_aligned_epoch: u64
}
```

This group owns dynamic external tool visibility and staged mutation.

### `drain`

```text
drain = {
  phase: Inactive | Starting | Running | ExitedRespawnable | Stopped,
  mode: Option<CommsDrainMode>
}
```

This group owns keep-alive/comms-drain lifecycle for the runtime.

## Derived Views

These are useful, but should not become new semantic owners:

- `runtime_is_live == binding.driver_present /\ control.phase \notin {Stopped, Destroyed}`
- `runtime_can_accept_new_input == control.phase \in {Idle, Attached, Running}`
- `runtime_can_process_queue == control.phase \in {Idle, Attached, Retired}`
- `queued_inputs == { w \in inputs.admitted : inputs.lifecycle[w] = Queued }`
- `staged_inputs == SeqToSet(inputs.current_run_contributors)`
- `active_tool_surfaces == tools.visible_surfaces`
- `ops_wait_satisfied == ops.wait_active /\ AllTerminal(ops.wait_operation_ids)`

The rule is: if a view can be recomputed from owner state, it should remain a
view.

## External Input Alphabet

The combined Meerkat machine accepts inputs from these sources:

- runtime/control-plane commands
  - `Initialize`, `AttachExecutor`, `DetachExecutor`, `RetireRequested`,
    `RecycleRequested`, `ResetRequested`, `RecoverRequested`, `StopRequested`,
    `DestroyRequested`, `ResumeRequested`
- admitted work inputs
  - `SubmitWork`, `AdmitQueued`, `AdmitConsumedOnAccept`,
    `SupersedeQueuedInput`, `CoalesceQueuedInputs`, `SetSilentIntentOverrides`
- run and turn events
  - `BeginRun`, `PrimitiveApplied`, `LlmReturnedToolCalls`,
    `RegisterPendingOps`, `ToolCallsResolved`, `OpsBarrierSatisfied`,
    `BoundaryContinue`, `BoundaryComplete`, terminal/failure/cancel events
- async-operation lifecycle events
  - `RegisterOperation`, `ProgressReported`, `CompleteOperation`,
    `FailOperation`, `CancelOperation`, `BeginWaitAll`, `CollectTerminal`
- peer ingress events
  - `TrustPeer`, `ReceivePeerEnvelope`, `SubmitTypedPeerInput`
- external tool-surface lifecycle events
  - `StageAdd`, `StageRemove`, `StageReload`, `ApplyBoundary`,
    `PendingSucceeded`, `PendingFailed`, `CallStarted`, `CallFinished`,
    `FinalizeRemovalClean`, `FinalizeRemovalForced`, `SnapshotAligned`
- drain lifecycle callbacks
  - `EnsureRunning`, `TaskSpawned`, `TaskExited`, `AbortObserved`

## Combined Transition Families

These are the transition families the internal Meerkat sketch should expose.

| Family | Typical triggering inputs | Regions updated | Notes |
| --- | --- | --- | --- |
| Binding lifecycle | `Initialize`, registration, attachment publication, `RecoverRequested`, `DestroyRequested` | `binding`, `control`, sometimes `drain` | Creates or rebinds the hidden runtime skeleton. |
| Control-plane lifecycle | `RetireRequested`, `RecycleRequested`, `ResetRequested`, `StopRequested`, `ResumeRequested`, run terminal events | `control` and often `inputs` or `turn` | Out-of-band control must preempt ordinary queue work. |
| Work admission | `SubmitWork`, `AdmitQueued`, `AdmitConsumedOnAccept`, peer-originated admitted work | `inputs`, `control` | Decides queue/steer/consume-on-accept and schedules wake/process intent. |
| Queue staging and run start | `StageDrainSnapshot`, `BeginRun`, `StartConversationRun`, `StartImmediateAppend`, `StartImmediateContext` | `inputs`, `control`, `turn` | This is one logical cross-region transition, even if current code routes it. |
| Turn loop execution | `PrimitiveApplied`, `LlmReturnedToolCalls`, `LlmReturnedTerminal`, `BoundaryContinue`, `BoundaryComplete`, extraction and retry inputs | `turn`, sometimes `inputs` and `tools` | Owns LLM/tool loop and boundary emission. |
| Async-op lifecycle | `RegisterOperation`, `ProgressReported`, `CompleteOperation`, `FailOperation`, `CancelOperation`, `BeginWaitAll` | `ops`, sometimes `turn` | Barrier and wait truth come from here. |
| Peer normalization | `TrustPeer`, `ReceivePeerEnvelope`, `SubmitTypedPeerInput` | `peer`, then `inputs` | Valid peer work becomes ordinary admitted input. |
| Tool-surface mutation | `StageAdd`, `StageRemove`, `StageReload`, `ApplyBoundary`, pending completions | `tools`, sometimes `turn` | Boundary-apply is the coupling point to execution. |
| Drain lifecycle | `EnsureRunning`, `TaskSpawned`, `TaskExited`, `StopRequested`, `AbortObserved` | `drain`, sometimes `binding` and `control` | Long-lived background lifecycle that must track runtime liveness. |
| Recovery and reconstruction | `RecoverRequested`, `Recover`, registration cold path, store replay | all regions | Recovery is not a side feature; it rebuilds the whole machine. |

## Representative Cross-Region Transitions

### 1. Admit queued work

```text
AdmitQueued(work_id, handling_mode, metadata):
  PRE:
    control.phase \in {Idle, Attached, Running}
    work_id \notin inputs.admitted
  UPDATES:
    inputs.admitted' = SetUnion(inputs.admitted, {work_id})
    inputs.lifecycle'[work_id] = Queued
    inputs.content_shape'[work_id] = metadata.content_shape
    inputs.request_id'[work_id] = metadata.request_id
    inputs.reservation_key'[work_id] = metadata.reservation_key
    inputs.policy_snapshot'[work_id] = metadata.policy
    inputs.handling_mode'[work_id] = handling_mode
    IF handling_mode = Queue THEN
      inputs.queue' = Append(inputs.queue, work_id)
    ELSE
      inputs.steer_queue' = Append(inputs.steer_queue, work_id)
      control.process_pending' = TRUE
    control.wake_pending' = TRUE
```

This transition shows that admission is not just queue mutation. It also
creates lifecycle truth and control-plane wake intent.

### 2. Stage contributors and begin a run

```text
BeginRun(run_id, contributors):
  PRE:
    control.phase \in {Idle, Attached, Retired, Recovering}
    control.current_run_id = None
    contributors # << >>
    contributors = CurrentDrainPrefix(inputs)
  UPDATES:
    control.pre_run_phase' = control.phase
    control.phase' = Running
    control.current_run_id' = run_id
    inputs.current_run_id' = run_id
    inputs.current_run_contributors' = contributors
    inputs.queue' = RemovePrefix(inputs.queue, contributors)
    inputs.steer_queue' = RemovePrefix(inputs.steer_queue, contributors)
    inputs.lifecycle'[c in contributors] = Staged
    inputs.last_run_id'[c in contributors] = run_id
    turn.phase' = ApplyingPrimitive
    turn.active_run_id' = run_id
```

This is one of the main places where today's separate machines still behave as
one semantic action.

### 3. Barrier satisfaction and boundary application

```text
OpsBarrierSatisfied(run_id, operation_ids):
  PRE:
    turn.phase = WaitingForOps
    turn.active_run_id = run_id
    operation_ids = turn.barrier_operation_ids
    AllTerminal(operation_ids, ops.operation_status)
  UPDATES:
    turn.barrier_satisfied' = TRUE
    turn.phase' = DrainingBoundary

BoundaryComplete(run_id, boundary_sequence):
  PRE:
    turn.phase = DrainingBoundary
    turn.active_run_id = run_id
    inputs.current_run_id = run_id
  UPDATES:
    turn.boundary_count' = turn.boundary_count + 1
    inputs.lifecycle'[c in inputs.current_run_contributors] = AppliedPendingConsumption
    inputs.last_boundary_sequence'[c in inputs.current_run_contributors] = boundary_sequence
    tools = ApplyStagedToolBoundary(tools)
```

This is the core coupling between `turn`, `ops`, `inputs`, and `tools`.

### 4. Run terminalization

```text
RunCompleted(run_id):
  PRE:
    control.current_run_id = run_id
    turn.active_run_id = run_id
  UPDATES:
    turn.phase' = Completed
    turn.terminal_outcome' = Completed
    inputs.lifecycle'[c in inputs.current_run_contributors] = Consumed
    inputs.terminal_outcome'[c in inputs.current_run_contributors] = Consumed
    inputs.current_run_id' = None
    inputs.current_run_contributors' = << >>
    control.current_run_id' = None
    control.phase' = control.pre_run_phase
```

Failure and cancellation variants follow the same shape, but write different
terminal outcomes and may trigger abandonment or waiter resolution.

### 5. Reset runtime

```text
ResetRuntime:
  PRE:
    control.phase \neq Running
  UPDATES:
    control.phase' = Idle
    control.current_run_id' = None
    control.pre_run_phase' = None
    control.wake_pending' = FALSE
    control.process_pending' = FALSE
    inputs = ResetInputRegion(inputs)          \* abandons queued/staged work
    turn = ResetTurnRegion(turn)
    ops = ResetOpsRegion(ops)                  \* terminalize or clear live wait state
    peer = ResetPeerSubmissionState(peer)      \* keep trust, clear transient submission lineage
    tools = PreserveOrRebuildToolSurface(tools)
    drain = StopDrainIfRequired(drain)
```

This is intentionally broad. Reset is one of the places where separate local
owners historically leave stale truth behind.

### 6. Recover runtime

```text
RecoverRuntime(snapshot):
  PRE:
    control.phase \in {Initializing, Recovering, Idle, Attached}
  UPDATES:
    binding.epoch_id' = RecoverOrRotateEpoch(snapshot)
    binding.cursor_state' = RecoverCursorState(snapshot)
    ops' = RecoverOps(snapshot)
    inputs' = RecoverInputs(snapshot)
    control' = RecoverControl(snapshot, inputs')
    turn' = RecoverTurn(snapshot, control', inputs', ops')
    peer' = RecoverPeerIngress(snapshot)
    tools' = RecoverToolSurface(snapshot)
    drain' = RecoverDrainSlots(snapshot, control')
```

The exact recovery policy is open, but the structural point is fixed: recovery
is a whole-machine reconstruction problem.

## Candidate Global Invariants

- `binding.driver_present = FALSE => control.phase \in {Initializing, Destroyed}`
- `control.phase = Running => control.current_run_id # None`
- `control.current_run_id # None => control.phase \in {Running, Retired}`
- `control.current_run_id = inputs.current_run_id` whenever either is non-`None`
- `turn.active_run_id = control.current_run_id` whenever `turn.phase \notin {Ready, Completed, Failed, Cancelled}`
- `inputs.queue` and `inputs.steer_queue` contain only inputs whose lifecycle is `Queued`
- `inputs.current_run_contributors` contains only inputs whose lifecycle is `Staged`, `Applied`, or `AppliedPendingConsumption`
- terminal inputs never reappear in `inputs.queue` or `inputs.steer_queue`
- `turn.phase = WaitingForOps => turn.pending_op_refs # None`
- `turn.barrier_satisfied => AllTerminal(turn.barrier_operation_ids, ops.operation_status)`
- `peer.trusted_peers` changes only through one runtime trust-registration
  transition, not raw router mutation
- `peer.phase \in {Absent, Dropped, Delivered} => Len(peer.submission_queue) = 0`
- `peer.phase = Received => Len(peer.submission_queue) > 0`
- every `peer.submission_queue` item is trusted, classified, and correlation-preserving
- `SubsetEq(tools.visible_surfaces, tools.known_surfaces)`
- removing or removed tool surfaces are not visible
- tool-surface pending ops are compatible with base state
- inflight tool-surface calls only exist for `Active | Removing`
- forced tool-surface delta phase implies remove delta operation
- staged tool-surface sequences exist iff staged op is not `None`
- pending tool-surface task/lineage sequences exist iff pending op is not `None`
- tool-surface removal timing exists iff base state is `Removing`
- `drain.phase \in {Starting, Running, ExitedRespawnable} => drain.mode # None`

The current executable validator in `meerkat/src/meerkat_machine.rs` checks a
conservative subset of these invariants against the live joined snapshot:

- `control.phase = Running => control.current_run_id # None`
- `control.current_run_id # None => control.phase \in {Running, Retired}`
- `control.current_run_id = inputs.current_run_id`
- `inputs.current_run_id = None <=> inputs.current_run_contributors = << >>`
- queued and steer-queued inputs must exist in the admitted ledger with
  lifecycle `Queued`
- current run contributors must exist in the admitted ledger with lifecycle in
  `{Staged, Applied, AppliedPendingConsumption}`
- terminal admitted inputs must carry terminal outcomes, and non-terminal
  admitted inputs must not
- `turn.active_run_id = control.current_run_id` for non-ready, non-terminal
  turn phases
- `turn.phase = WaitingForOps => turn.pending_op_refs # None`
- peer authority phase and submission queue coherence
- `ops.active_count <= operation_count`
- completion-waiter carrier counts are internally consistent and every waiting
  input still exists in the admitted ledger
- tool-surface visibility matches `Active` base membership
- tool-surface pending op/base-state compatibility
- tool-surface inflight/base-state compatibility
- forced tool-surface delta phase implies remove delta operation
- staged tool-surface sequence coherence
- pending tool-surface task/lineage sequence coherence
- tool-surface removal-timing membership coherence

## Resolved Target Formalization Decisions

The top-level target freeze now answers the main formalization questions:

- `ResetRuntime` rotates `binding.epoch_id`
- completion-waiter resolution is explicit machine state, not implicit support
  carrier behavior
- trust membership is machine-owned recoverable state
- tool-surface rebuild belongs to the Meerkat machine, not to a second
  out-of-band recovery protocol
- `ResetInputRegion`, `ResetOpsRegion`, and `RecoverTurn` are formalized to
  match the target lifecycle commitments in `meerkat-machine-freeze.md`

## Relationship To Existing Docs

- `meerkat-kernel-shape.md` names the Meerkat regions and why they form one
  kernel
- `meerkat-owned-facts-ledger.md` inventories the facts and carriers those
  regions must own
- this document sketches the combined state variables and transitions that bind
  those facts together
