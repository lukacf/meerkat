# MobMachine State Schema

Status: frozen target-state state handoff

This note is the canonical target-state schema for `MobMachine`.

Use it with:

- `mob-machine-freeze.md`
- `mob-machine-transition-catalog.md`
- `mob-machine-proof-obligations.md`
- `mob-machine-glossary.md`

## Abstract Scalar Domains

The target model treats these as finite abstract domains rather than as
implementation-precision values.

### Identity domains

- `AgentIdentity`
- `Generation`
- `AgentRuntimeId`
- `FenceToken`
- `BindingRef`

### Flow domains

- `FlowId`
- `RunId`
- `StepId`
- `FrameId`
- `LoopId`
- `LoopInstanceId`
- `BranchId`
- `WorkRef`
- `TaskId`

### Coordination domains

- `ProfileId`
- `RuntimeMode`
- `RestoreReason`
- `EventSeq`

### Dispatch / collection domains

- `DispatchMode`
- `CollectionPolicyKind`

## Top-Level Shape

```text
MobMachine =
  identity
  + lifecycle
  + roster
  + topology
  + provisioning
  + work
  + flows
  + tasks
  + recovery
  + history
```

## Region Schemas

### `identity`

```text
identity = {
  known: SUBSET AgentIdentity,
  generation: [AgentIdentity -> Nat],
  runtime_id: [AgentIdentity -> Option<AgentRuntimeId>],
  fence_token: [AgentIdentity -> Nat],
  authority: [AgentIdentity -> AuthorityState],
  binding_present: [AgentIdentity -> BOOLEAN],
  checkpoint_version: [AgentIdentity -> Nat]
}
```

#### `AuthorityState`

```text
AuthorityState =
  Absent
  | Provisioning
  | Active
  | Retiring
  | Destroyed
```

### `lifecycle`

```text
lifecycle = {
  phase: MobPhase,
  active_run_count: Nat,
  cleanup_pending: Bool
}
```

#### `MobPhase`

```text
MobPhase = Creating | Running | Stopped | Completed | Destroyed
```

### `roster`

```text
roster = {
  present: SUBSET AgentIdentity,
  profile: [AgentIdentity -> Option<ProfileId>],
  runtime_mode: [AgentIdentity -> Option<RuntimeMode>],
  labels_present: [AgentIdentity -> BOOLEAN],
  member_status: [AgentIdentity -> MemberStatus],
  runtime_binding_live: [AgentIdentity -> BOOLEAN]
}
```

#### `MemberStatus`

```text
MemberStatus = Pending | Active | Retiring | Broken | Completed | Destroyed
```

### `topology`

```text
topology = {
  coordinator: Option<AgentIdentity>,
  revision: Nat,
  edges: SUBSET (AgentIdentity \X AgentIdentity),
  external_specs_present: SUBSET AgentIdentity
}
```

### `provisioning`

```text
provisioning = {
  pending_spawn: SUBSET AgentIdentity,
  pending_runtime_id: [AgentIdentity -> Option<AgentRuntimeId>],
  kickoff_pending: SUBSET AgentIdentity,
  restore_failed: SUBSET AgentIdentity,
  restore_reason_present: [AgentIdentity -> BOOLEAN]
}
```

`pending_spawn` overlaps `roster.present` during `ResetMember` while the
identity remains logically part of the mob before finalization of its new
runtime incarnation.

`kickoff_pending` is the autonomous kickoff barrier set. Members in this set
are already finalized into the roster, have a live runtime binding, and are no
longer part of pending-spawn lineage.

### `work`

```text
work = {
  known: SUBSET WorkRef,
  owner: [WorkRef -> Option<AgentIdentity>],
  runtime_target: [WorkRef -> Option<AgentRuntimeId>],
  run_target: [WorkRef -> Option<RunId>],
  step_target: [WorkRef -> Option<StepId>],
  status: [WorkRef -> WorkStatus],
  terminal: [WorkRef -> Option<WorkTerminalOutcome>]
}
```

#### `WorkStatus`

```text
WorkStatus =
  Pending
  | Accepted
  | Rejected
  | Running
  | Completed
  | Failed
  | Canceled
```

#### `WorkTerminalOutcome`

```text
WorkTerminalOutcome = WorkCompleted | WorkFailed | WorkCanceled | WorkRejected
```

### `flows`

```text
flows = {
  known_runs: SUBSET RunId,
  tracked_runs: SUBSET RunId,
  cancel_tokens: SUBSET RunId,
  stream_runs: SUBSET RunId,
  flow_id: [RunId -> Option<FlowId>],
  status: [RunId -> Option<RunStatus>],
  schema_version: [RunId -> Nat],
  completed_at_present: [RunId -> BOOLEAN],
  frame_count: [RunId -> Nat],
  loop_count: [RunId -> Nat],
  loop_iteration_count: [RunId -> Nat],
  ordered_steps: [RunId -> Seq(StepId)],
  step_dependencies: [RunId -> [StepId -> SUBSET StepId]],
  step_dispatch_mode: [RunId -> [StepId -> DispatchMode]],
  step_dispatch_width: [RunId -> [StepId -> Nat]],
  step_dependency_mode: [RunId -> [StepId -> DependencyMode]],
  step_has_condition: [RunId -> [StepId -> BOOLEAN]],
  step_branch: [RunId -> [StepId -> Option<BranchId>]],
  step_collection_policy: [RunId -> [StepId -> CollectionPolicyKind]],
  step_quorum_threshold: [RunId -> [StepId -> Nat]],
  step_collection_observed: [RunId -> [StepId -> Nat]],
  step_status: [RunId -> [StepId -> Option<StepStatus>]],
  failure_count: [RunId -> Nat],
  consecutive_failure_count: [RunId -> Nat],
  max_step_retries: [RunId -> Nat],
  escalation_threshold: [RunId -> Nat]
}
```

#### `RunStatus`

```text
RunStatus = Pending | Running | Completed | Failed | Canceled
```

#### `StepStatus`

```text
StepStatus = Pending | Dispatched | Completed | Failed | Skipped | Canceled
```

#### `DependencyMode`

```text
DependencyMode = All | Any
```

#### `DispatchMode`

```text
DispatchMode = FanOut | OneToOne | FanIn
```

#### `CollectionPolicyKind`

```text
CollectionPolicyKind = All | Any | Quorum
```

`step_status = Pending` is reserved for machine-owned ready state after an
`Any` join resolves but before member work has been dispatched. `All`-mode
dependency satisfaction dispatches directly from `None` once dependencies are
satisfied.

The target machine also fixes these coupling rules:

- `Pending` steps carry no bound work yet
- `Dispatched` steps retain at least one bound work record
- terminal step states do not retain live bound work
- terminal run states do not retain live bound work

Dispatch mode is also explicit machine truth. The target machine does not hide
dispatch-family semantics in helper code:

- `OneToOne` = at most one selected target contributes
- `FanIn` = multi-target aggregation with array-style aggregate shape
- `FanOut` = multi-target dispatch with keyed aggregate shape unless
  `CollectionPolicy = Any`, in which case the aggregate shape is scalar-like

`StartFlowRun` seeds an explicit per-step dispatch-mode map. Dispatch family is
not recovered later from helper behavior or from the flow archetype name.

`step_dispatch_width` is not a planned width. It is machine-owned execution
truth: `0` before dispatch, then the actual number of work items bound when
`DispatchStep` materializes a step as `Dispatched`.

For quorum collection steps, `step_collection_observed` is explicit machine
state. `Quorum` completion requires observed contribution count to reach the
stored threshold before the step resolves to `Completed`.

### `tasks`

```text
tasks = {
  known: SUBSET TaskId,
  status: [TaskId -> TaskStatus],
  owner: [TaskId -> Option<AgentIdentity>],
  run_binding: [TaskId -> Option<RunId>]
}
```

#### `TaskStatus`

```text
TaskStatus = Pending | InProgress | Blocked | Completed | Canceled
```

Live task bindings reference only non-terminal runs. Terminal task rows do not
retain a live run binding.

### `recovery`

```text
recovery = {
  restore_failures: SUBSET AgentIdentity,
  checkpoint_version: [AgentIdentity -> Nat],
  continuity_bound: [AgentIdentity -> BOOLEAN]
}
```

### `history`

```text
history = {
  next_seq: Nat,
  event_count_by_identity: [AgentIdentity -> Nat],
  last_seq_by_identity: [AgentIdentity -> Nat]
}
```

`next_seq` is the global durable event frontier. `event_count_by_identity` and
`last_seq_by_identity` are per-identity local sequence/count truth, and in the
target machine they advance together for the same identity on every appended
event.

## Target `Init`

The canonical target `Init` is:

- no known identities
- `lifecycle.phase = Creating`
- empty roster
- empty topology
- no pending spawns
- no known work
- no known runs
- no tasks
- no restore failures
- `history.next_seq = 0`

All per-identity and per-run maps start in their zero / absent state.

## Minimum `TypeOK` Commitments

The target proof baseline must enforce at least:

- `roster.present ⊆ identity.known`
- `provisioning.pending_spawn ⊆ identity.known`
- `provisioning.kickoff_pending ⊆ roster.present`
- `recovery.restore_failures ⊆ identity.known`
- `flows.tracked_runs ⊆ flows.known_runs`
- `flows.cancel_tokens ⊆ flows.known_runs`
- `flows.stream_runs ⊆ flows.known_runs`
- `work.run_target[w] = Some(r) => r ∈ flows.known_runs`
- `work.owner[w] = Some(id) => id ∈ identity.known`
- `work.step_target[w] = Some(s) => work.run_target[w] = Some(r) ∧ s ∈ KnownSteps(r)`
- every ordered step has one explicit dispatch-mode entry
- `flows.step_collection_policy[r][s] = Quorum => flows.step_quorum_threshold[r][s] > 0`
- `flows.step_status[r][s] = Completed ∧ flows.step_collection_policy[r][s] = Quorum => flows.step_collection_observed[r][s] >= flows.step_quorum_threshold[r][s]`
- `topology.coordinator = Some(id) => id ∈ roster.present`
- `topology.edges ⊆ roster.present × roster.present`
- `history.last_seq_by_identity[id] <= history.next_seq`
