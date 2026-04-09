# MeerkatMachine State Schema

Status: frozen target-state state handoff

This note is the canonical target-state schema for `MeerkatMachine`.

Use it with:

- `meerkat-machine-freeze.md`
- `meerkat-machine-transition-catalog.md`
- `meerkat-machine-proof-obligations.md`
- `meerkat-machine-glossary.md`

## Purpose

The freeze package already fixes the target boundary, transitions, and proof
obligations. This note adds the canonical state schema needed for TLA+ work:

- region contents
- allowed domains
- initial state
- region-local typing constraints

Without this note, a modeler would still be choosing some domain details while
writing `Init` and `TypeOK`.

## Abstract Scalar Domains

The target model treats these as finite abstract domains rather than as
implementation-precision values.

### Identity domains

- `SessionId`
- `RuntimeId`
- `EpochId`
- `InputId`
- `RunId`
- `OpId`
- `PeerId`
- `PeerItemId`
- `SurfaceId`
- `WaitRequestId`
- `RequestId`
- `ReservationKey`
- `BoundarySequence`

These domains require only distinctness and equality, except `EpochId` and
`BoundarySequence`, which also require an abstract “advances from previous”
ordering relation.

### Content / classification domains

```text
ContentShape = TextOnly | InlineImage | MixedMedia | PeerPayload
```

```text
TurnPrimitiveKind =
  ConversationRun
  | ImmediateAppend
  | ImmediateContext
  | PeerTurn
  | DetachedContinuation
```

```text
TurnTerminalOutcome = TurnCompleted | TurnFailed | TurnCancelled
```

```text
InputTerminalOutcome =
  InputConsumed
  | InputSuperseded
  | InputCoalesced
  | InputAbandoned
  | InputCancelled
  | InputFailed
```

```text
OperationKind =
  BackgroundToolOp
  | PeerReadyOp
  | MobMemberChildOp
  | ToolMutationOp
```

```text
OperationTerminalOutcome =
  OperationSucceeded
  | OperationFailed
  | OperationCancelled
```

### Policy / admission domains

```text
PolicyDecision = PromptAdmission | PromptRejection | PromptCoalesced
```

## Top-Level Shape

```text
MeerkatMachine =
  binding
  + control
  + inputs
  + completion
  + turn
  + ops
  + peer
  + tools
  + drain
```

## Region Schemas

### `binding`

```text
binding = {
  live: Bool,
  session_id: Option<SessionId>,
  runtime_id: Option<RuntimeId>,
  driver_present: Bool,
  attachment_live: Bool,
  epoch_id: Option<EpochId>,
  cursor_state: CursorState
}
```

#### Domain commitments

- `live = FALSE` iff lifecycle has reached destroyed state
- `session_id`, `runtime_id`, and `epoch_id` are present iff `live = TRUE`
- `attachment_live => driver_present`

#### `CursorState`

```text
CursorState = {
  agent_applied_cursor: Nat,
  runtime_observed_seq: Nat,
  runtime_last_injected_seq: Nat
}
```

```text
ZeroCursorState = {
  agent_applied_cursor: 0,
  runtime_observed_seq: 0,
  runtime_last_injected_seq: 0
}
```

### `control`

```text
control = {
  phase: RuntimePhase,
  current_run_id: Option<RunId>,
  pre_run_phase: Option<RuntimePhase>,
  wake_pending: Bool,
  process_pending: Bool,
  pending_control: SUBSET ControlCommand
}
```

#### `RuntimePhase`

```text
RuntimePhase =
  Initializing
  | Idle
  | Attached
  | Running
  | Recovering
  | Retired
  | Stopped
  | Destroyed
```

#### `ControlCommand`

```text
ControlCommand =
  CancelCurrentRun
  | InterruptYielding
  | StopRuntimeExecutor
```

### `inputs`

```text
inputs = {
  admitted: SUBSET InputId,
  content_shape: [InputId -> Option<ContentShape>],
  request_id: [InputId -> Option<RequestId>],
  reservation_key: [InputId -> Option<ReservationKey>],
  handling_mode: [InputId -> Option<HandlingMode>],
  lifecycle: [InputId -> Option<InputLifecycleState>],
  terminal_outcome: [InputId -> Option<InputTerminalOutcome>],
  queue: Seq(InputId),
  steer_queue: Seq(InputId),
  current_run_id: Option<RunId>,
  current_run_contributors: Seq(InputId),
  last_run_id: [InputId -> Option<RunId>],
  last_boundary_sequence: [InputId -> Option<BoundarySequence>]
}
```

#### `HandlingMode`

```text
HandlingMode = Queue | Steer
```

#### `InputLifecycleState`

```text
InputLifecycleState =
  Queued
  | Staged
  | AppliedPendingConsumption
  | Consumed
  | Superseded
  | Coalesced
  | Abandoned
  | Cancelled
  | Failed
```

### `completion`

```text
completion = {
  waiting_inputs: SUBSET InputId,
  waiter_count_by_input: [InputId -> Nat],
  resolved_inputs: SUBSET InputId
}
```

#### Domain commitments

- `resolved_inputs ⊆ inputs.admitted`
- `waiting_inputs ∩ resolved_inputs = {}`

### `turn`

```text
turn = {
  phase: TurnPhase,
  active_run_id: Option<RunId>,
  primitive_kind: TurnPrimitiveKind,
  tool_calls_pending: Nat,
  pending_op_refs: SUBSET OpId,
  barrier_operation_ids: SUBSET OpId,
  barrier_satisfied: Bool,
  cancel_after_boundary: Bool,
  extraction_attempts: Nat,
  max_extraction_retries: Nat,
  terminal_outcome: Option<TurnTerminalOutcome>
}
```

#### `TurnPhase`

```text
TurnPhase =
  Ready
  | ApplyingPrimitive
  | CallingLlm
  | WaitingForOps
  | DrainingBoundary
  | Extracting
  | ErrorRecovery
  | Cancelling
  | Completed
  | Failed
  | Cancelled
```

### `ops`

```text
ops = {
  known_operations: SUBSET OpId,
  operation_status: [OpId -> Option<OperationStatus>],
  operation_kind: [OpId -> Option<OperationKind>],
  watcher_count: [OpId -> Nat],
  progress_count: [OpId -> Nat],
  peer_ready: [OpId -> Bool],
  terminal_outcome: [OpId -> Option<OperationTerminalOutcome>],
  terminal_buffered: [OpId -> Bool],
  completed_order: Seq(OpId),
  active_count: Nat,
  wait_active: Bool,
  wait_request_id: Option<WaitRequestId>,
  wait_operation_ids: SUBSET OpId
}
```

#### `OperationStatus`

```text
OperationStatus =
  Pending
  | Running
  | Succeeded
  | Failed
  | Cancelled
```

### `peer`

```text
peer = {
  auth_required: Bool,
  trusted_peers: SUBSET PeerId,
  backlog: Seq(PeerItemId),
  replayable: SUBSET PeerItemId,
  terminalized_items: SUBSET PeerItemId,
  admitted_items: SUBSET PeerItemId,
  classification: [PeerItemId -> Option<PeerItemClass>],
  correlation: [PeerItemId -> Option<RequestId>],
  trust_snapshot: [PeerItemId -> Option<Bool>]
}
```

#### `PeerItemClass`

```text
PeerItemClass = PeerWork | PeerRequest | PeerResponse | PeerEvent
```

### `tools`

```text
tools = {
  known_surfaces: SUBSET SurfaceId,
  visible_surfaces: SUBSET SurfaceId,
  base_state: [SurfaceId -> Option<SurfaceBaseState>],
  staged_op: [SurfaceId -> Option<SurfaceMutationOp>],
  pending_op: [SurfaceId -> Option<SurfaceMutationOp>],
  inflight_calls: [SurfaceId -> Nat],
  snapshot_epoch: Nat,
  snapshot_aligned_epoch: Nat
}
```

#### `SurfaceBaseState`

```text
SurfaceBaseState = Active | Removing
```

#### `SurfaceMutationOp`

```text
SurfaceMutationOp = Add | Remove | Reload
```

### `drain`

```text
drain = {
  mode: Option<DrainMode>,
  phase: DrainPhase,
  suppressed: Bool,
  respawn_required: Bool
}
```

#### `DrainMode`

```text
DrainMode =
  PersistentHost
  | EphemeralHost
  | AttachedHost
  | Disabled
```

#### `DrainPhase`

```text
DrainPhase =
  Inactive
  | Starting
  | Running
  | ExitedRespawnable
  | Stopped
```

## Initial State

The target machine initial state is a pre-binding empty runtime.

```text
Init ==
  /\ binding.live = FALSE
  /\ binding.session_id = None
  /\ binding.runtime_id = None
  /\ binding.driver_present = FALSE
  /\ binding.attachment_live = FALSE
  /\ binding.epoch_id = None
  /\ binding.cursor_state = ZeroCursorState
  /\ control.phase = Initializing
  /\ control.current_run_id = None
  /\ control.pre_run_phase = None
  /\ control.wake_pending = FALSE
  /\ control.process_pending = FALSE
  /\ control.pending_control = {}
  /\ inputs.admitted = {}
  /\ inputs.content_shape = [i \in InputId |-> None]
  /\ inputs.request_id = [i \in InputId |-> None]
  /\ inputs.reservation_key = [i \in InputId |-> None]
  /\ inputs.handling_mode = [i \in InputId |-> None]
  /\ inputs.lifecycle = [i \in InputId |-> None]
  /\ inputs.terminal_outcome = [i \in InputId |-> None]
  /\ inputs.queue = << >>
  /\ inputs.steer_queue = << >>
  /\ inputs.current_run_id = None
  /\ inputs.current_run_contributors = << >>
  /\ inputs.last_run_id = [i \in InputId |-> None]
  /\ inputs.last_boundary_sequence = [i \in InputId |-> None]
  /\ completion.waiting_inputs = {}
  /\ completion.waiter_count_by_input = [i \in InputId |-> 0]
  /\ completion.resolved_inputs = {}
  /\ turn.phase = Ready
  /\ turn.active_run_id = None
  /\ turn.primitive_kind = ConversationRun
  /\ turn.tool_calls_pending = 0
  /\ turn.pending_op_refs = {}
  /\ turn.barrier_operation_ids = {}
  /\ turn.barrier_satisfied = FALSE
  /\ turn.cancel_after_boundary = FALSE
  /\ turn.extraction_attempts = 0
  /\ turn.max_extraction_retries = 0
  /\ turn.terminal_outcome = None
  /\ ops.known_operations = {}
  /\ ops.operation_status = [o \in OpId |-> None]
  /\ ops.operation_kind = [o \in OpId |-> None]
  /\ ops.watcher_count = [o \in OpId |-> 0]
  /\ ops.progress_count = [o \in OpId |-> 0]
  /\ ops.peer_ready = [o \in OpId |-> FALSE]
  /\ ops.terminal_outcome = [o \in OpId |-> None]
  /\ ops.terminal_buffered = [o \in OpId |-> FALSE]
  /\ ops.completed_order = << >>
  /\ ops.active_count = 0
  /\ ops.wait_active = FALSE
  /\ ops.wait_request_id = None
  /\ ops.wait_operation_ids = {}
  /\ peer.auth_required = FALSE
  /\ peer.trusted_peers = {}
  /\ peer.backlog = << >>
  /\ peer.replayable = {}
  /\ peer.classification = [p \in PeerItemId |-> None]
  /\ peer.correlation = [p \in PeerItemId |-> None]
  /\ peer.trust_snapshot = [p \in PeerItemId |-> None]
  /\ tools.known_surfaces = {}
  /\ tools.visible_surfaces = {}
  /\ tools.base_state = [s \in SurfaceId |-> None]
  /\ tools.staged_op = [s \in SurfaceId |-> None]
  /\ tools.pending_op = [s \in SurfaceId |-> None]
  /\ tools.inflight_calls = [s \in SurfaceId |-> 0]
  /\ tools.snapshot_epoch = 0
  /\ tools.snapshot_aligned_epoch = 0
  /\ drain.mode = None
  /\ drain.phase = Inactive
  /\ drain.suppressed = FALSE
  /\ drain.respawn_required = FALSE
```

## Region Typing Obligations

These are the minimum `TypeOK`-style obligations the schema imposes.

1. Queue and steer projections contain only admitted non-terminal inputs.
2. `current_run_contributors` contains only admitted inputs.
3. `completion.waiting_inputs ⊆ inputs.admitted`.
4. `turn.pending_op_refs ⊆ ops.known_operations`.
5. `turn.barrier_operation_ids ⊆ ops.known_operations`.
6. `ops.wait_operation_ids ⊆ ops.known_operations`.
7. `peer.replayable ⊆ SeqToSet(peer.backlog)`.
8. `peer.admitted_items ⊆ peer.terminalized_items`.
9. `peer.terminalized_items ∩ SeqToSet(peer.backlog) = {}`.
10. `peer.terminalized_items ∩ peer.replayable = {}`.
11. `tools.visible_surfaces ⊆ tools.known_surfaces`.
12. `tools.base_state` is defined only for known surfaces.
13. `binding.live = FALSE => drain.phase = Inactive`.

## Acceptance

This schema is complete enough for target-state TLA+ work when:

- every region in the target freeze has a canonical schema
- phase and status domains are enumerated
- the target machine has a canonical `Init`
- the minimum `TypeOK` obligations are explicit

That state is now reached.
