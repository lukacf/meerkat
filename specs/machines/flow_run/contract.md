# FlowRunMachine

Status: normative `0.5` machine contract, first formal-spec draft

## Purpose

`FlowRunMachine` owns one flow-run aggregate and its per-step execution
semantics.

It is the authoritative owner of:

- run lifecycle state
- per-step ledger state
- failure ledger accumulation
- step-output write eligibility
- run terminalization

It is **not** the owner of:

- top-level mob lifecycle
- roster/wiring ownership
- turn execution itself
- transcript projection of flow progress

## Scope Boundary

This machine begins at run creation and ends at run terminalization.

It is a run aggregate owner. Step execution may cross machine boundaries into
runtime admission / turn execution, but the durable run truth remains here.

## Authoritative State Model

For one flow run instance, the machine state is the tuple:

- `run_status: FlowRunStatus`
- `step_status: Map<StepId, StepRunStatus>`
- `output_recorded: Map<StepId, Bool>`
- `failure_count: u32`

`FlowRunStatus` is the closed state set:

- `Absent`
- `Pending`
- `Running`
- `Completed`
- `Failed`
- `Canceled`

Terminal states:

- `Completed`
- `Failed`
- `Canceled`

`StepRunStatus` is the closed per-step state set:

- `None`
- `Dispatched`
- `Completed`
- `Failed`
- `Skipped`
- `Canceled`

## Input Alphabet

The closed external input/command alphabet for this machine is:

- `CreateRun`
- `StartRun`
- `DispatchStep(step_id)`
- `CompleteStep(step_id)`
- `RecordStepOutput(step_id)`
- `FailStep(step_id)`
- `SkipStep(step_id)`
- `CancelStep(step_id)`
- `TerminalizeCompleted`
- `TerminalizeFailed`
- `TerminalizeCanceled`

## Effect Family

The closed machine-boundary effect family is:

- `EmitFlowRunNotice(run_status)`
- `EmitStepNotice(step_id, step_status)`
- `AppendFailureLedger(step_id)`
- `PersistStepOutput(step_id)`
- `AdmitStepWork(step_id)`

Architecture rule:

- step-target dispatch into runtime/turn execution is an effect from this
  machine, not evidence that this machine executes turns locally

## Transition Relation

### Run lifecycle

1. `CreateRun`

Preconditions:

- `run_status = Absent`

State updates:

- `Absent -> Pending`

2. `StartRun`

Preconditions:

- `run_status = Pending`

State updates:

- `Pending -> Running`

### Step ledger mutation

3. `DispatchStep(step_id)`

Preconditions:

- `run_status = Running`
- `step_status[step_id] = None`

State updates:

- `None -> Dispatched`

Effect:

- `AdmitStepWork(step_id)`

4. `CompleteStep(step_id)`

Preconditions:

- `run_status = Running`
- `step_status[step_id] = Dispatched`

State updates:

- `Dispatched -> Completed`

5. `RecordStepOutput(step_id)`

Preconditions:

- `run_status = Running`
- `step_status[step_id] = Completed`
- `output_recorded[step_id] = FALSE`

State updates:

- `output_recorded[step_id] := TRUE`

Hard rule:

- step output may only be recorded after the step ledger already says
  `Completed`

6. `FailStep(step_id)`

Preconditions:

- `run_status = Running`
- `step_status[step_id] = Dispatched`

State updates:

- `Dispatched -> Failed`
- `failure_count += 1`

Effect:

- `AppendFailureLedger(step_id)`

7. `SkipStep(step_id)`

Preconditions:

- `run_status = Running`
- `step_status[step_id] = None`

State updates:

- `None -> Skipped`

8. `CancelStep(step_id)`

Preconditions:

- `run_status = Running`
- `step_status[step_id] ∈ {None, Dispatched}`

State updates:

- `-> Canceled`

### Run terminalization

9. `TerminalizeCompleted`

Preconditions:

- `run_status = Running`
- every step is in `Completed` or `Skipped`

State updates:

- `Running -> Completed`

10. `TerminalizeFailed`

Preconditions:

- `run_status = Running`
- no step remains `Dispatched`
- at least one step is `Failed`

State updates:

- `Running -> Failed`

11. `TerminalizeCanceled`

Preconditions:

- `run_status = Running`
- no step remains `Dispatched`
- at least one step is `Canceled`

State updates:

- `Running -> Canceled`

Hard rule:

- terminal run states reject all further transitions

## Invariants

The machine must maintain:

1. output only follows completed steps

- `output_recorded[step_id] = TRUE` implies `step_status[step_id] = Completed`

2. terminal runs have no dispatched steps

- once the run is terminal, no step may still be `Dispatched`

3. completed runs contain no failed or canceled steps

- `run_status = Completed` implies every step is `Completed` or `Skipped`

4. failure count is bounded below by failed-step presence

- a failed run implies at least one failed step or failure-ledger increment

## Model-check candidates

Best candidates for formal checking:

- output-write ordering
- terminal-state stickiness
- impossibility of terminal runs with dispatched steps still active
- completed terminalization cannot coexist with failed/canceled steps
