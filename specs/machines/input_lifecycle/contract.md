# InputLifecycleMachine

Status: normative `0.5` machine contract, first formal-spec draft

## Purpose

`InputLifecycleMachine` owns the lifecycle semantics of one admitted runtime
input after runtime admission has accepted it.

It is the authoritative owner of:

- per-input lifecycle state
- terminal outcome assignment
- run association for staged/applied work
- boundary receipt association after application
- legality of lifecycle transitions

It is **not** the owner of:

- runtime-wide queue ordering
- raw admission/rejection decisions
- policy resolution
- runtime control/state
- persistence mechanics

## Scope Boundary

This machine begins once a runtime input has been accepted into runtime-owned
processing.

It ends when the input reaches a terminal state:

- `Consumed`
- `Superseded`
- `Coalesced`
- `Abandoned`

Runtime-wide queueing, recovery, and multi-input sequencing belong to
`RuntimeIngressMachine`.

## Authoritative State Model

For one admitted input instance, the machine state is the tuple:

- `lifecycle_state: InputLifecycleState`
- `terminal_outcome: Option<InputTerminalOutcome>`
- `last_run_id: Option<RunId>`
- `last_boundary_sequence: Option<BoundarySequence>`

`InputLifecycleState` is the closed state set:

- `Accepted`
- `Queued`
- `Staged`
- `Applied`
- `AppliedPendingConsumption`
- `Consumed`
- `Superseded`
- `Coalesced`
- `Abandoned`

Terminal states:

- `Consumed`
- `Superseded`
- `Coalesced`
- `Abandoned`

## Input Alphabet

The closed external input/command alphabet for this machine is:

- `QueueAccepted`
- `StageForRun(run_id)`
- `RollbackStaged`
- `MarkApplied(run_id)`
- `MarkAppliedPendingConsumption(boundary_sequence)`
- `Consume`
- `Supersede`
- `Coalesce`
- `Abandon`

Notes:

- `QueueAccepted` corresponds to the runtime deciding that an accepted input
  should participate in ordinary queue/drain execution.
- `RollbackStaged` is the only rollback path; `AppliedPendingConsumption ->
  Queued` remains forbidden.
- Recovery logic in runtime ingress may decide whether to requeue, consume, or
  abandon an input, but those decisions must still refine this lifecycle
  relation rather than mutate state ad hoc.

## Effect Family

The closed machine-boundary effect family is:

- `InputLifecycleNotice(new_state)`
- `RecordTerminalOutcome(outcome)`
- `RecordRunAssociation(run_id)`
- `RecordBoundarySequence(boundary_sequence)`

Architecture rule:

- persistence writes, event emission, and transcript-visible notices are
  implementations or projections of these lifecycle effects, not substitutes
  for the machine contract itself

## Transition Relation

### Admission-to-queue progression

1. `QueueAccepted`

Preconditions:

- `lifecycle_state = Accepted`

State updates:

- `Accepted -> Queued`

2. `StageForRun(run_id)`

Preconditions:

- `lifecycle_state = Queued`

State updates:

- `Queued -> Staged`
- `last_run_id := run_id`

3. `RollbackStaged`

Preconditions:

- `lifecycle_state = Staged`

State updates:

- `Staged -> Queued`

### Run application and consumption

4. `MarkApplied(run_id)`

Preconditions:

- `lifecycle_state = Staged`
- `last_run_id = run_id`

State updates:

- `Staged -> Applied`

5. `MarkAppliedPendingConsumption(boundary_sequence)`

Preconditions:

- `lifecycle_state = Applied`

State updates:

- `Applied -> AppliedPendingConsumption`
- `last_boundary_sequence := boundary_sequence`

6. `Consume`

Preconditions:

- `lifecycle_state = AppliedPendingConsumption`

State updates:

- `AppliedPendingConsumption -> Consumed`
- `terminal_outcome := Consumed`

### Terminalization side paths

7. `Supersede`

Preconditions:

- `lifecycle_state ∈ {Accepted, Queued, Staged}`

State updates:

- `-> Superseded`
- `terminal_outcome := Superseded`

8. `Coalesce`

Preconditions:

- `lifecycle_state ∈ {Accepted, Queued}`

State updates:

- `-> Coalesced`
- `terminal_outcome := Coalesced`

9. `Abandon`

Preconditions:

- `lifecycle_state ∈ {Accepted, Queued, Staged, Applied, AppliedPendingConsumption}`

State updates:

- `-> Abandoned`
- `terminal_outcome := Abandoned`

Hard rule:

- terminal states reject all further transitions
- `AppliedPendingConsumption -> Queued` is forbidden

## Invariants

The machine must maintain:

1. terminal states are terminal

- `Consumed`, `Superseded`, `Coalesced`, and `Abandoned` reject all further
  lifecycle transitions

2. terminal outcome matches terminal state

- nonterminal states have no terminal outcome
- terminal states have the matching terminal outcome

3. boundary metadata only exists after application

- `last_boundary_sequence` may only be present once the input has crossed an
  apply boundary

4. accepted inputs carry no run/boundary metadata

- `Accepted` implies no run association and no boundary sequence

## Model-check candidates

Best candidates for formal checking:

- illegal lifecycle transitions are unreachable
- terminal states are sticky
- `AppliedPendingConsumption -> Queued` is impossible
- boundary metadata never appears before application

