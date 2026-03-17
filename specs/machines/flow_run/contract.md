# FlowRunMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `machines::flow_run`

## State
- Phase enum: `Absent | Pending | Running | Completed | Failed | Canceled`
- `tracked_steps`: `Set<StepId>`
- `step_status`: `Map<StepId, StepRunStatus>`
- `output_recorded`: `Map<StepId, Bool>`
- `failure_count`: `u32`

## Inputs
- `CreateRun`(step_ids: Seq<StepId>)
- `StartRun`
- `DispatchStep`(step_id: StepId)
- `CompleteStep`(step_id: StepId)
- `RecordStepOutput`(step_id: StepId)
- `FailStep`(step_id: StepId)
- `SkipStep`(step_id: StepId)
- `CancelStep`(step_id: StepId)
- `TerminalizeCompleted`
- `TerminalizeFailed`
- `TerminalizeCanceled`

## Effects
- `EmitFlowRunNotice`(run_status: FlowRunStatus)
- `EmitStepNotice`(step_id: StepId, step_status: StepRunStatus)
- `AppendFailureLedger`(step_id: StepId)
- `PersistStepOutput`(step_id: StepId)
- `AdmitStepWork`(step_id: StepId)
- `FlowTerminalized`(run_status: FlowRunStatus)

## Helpers
- `RunIsTerminal`() -> `Bool`
- `StepIsTracked`(step_id: StepId) -> `Bool`
- `StepStatusIs`(step_id: StepId, expected_status: String) -> `Bool`
- `StepOutputRecordedIs`(step_id: StepId, expected: Bool) -> `Bool`
- `AllTrackedStepsInAllowedStatuses`(allowed_statuses: Seq<String>) -> `Bool`
- `NoTrackedStepInStatus`(status: String) -> `Bool`
- `AnyTrackedStepInStatus`(status: String) -> `Bool`

## Invariants
- `output_only_follows_completed_steps`
- `terminal_runs_have_no_dispatched_steps`
- `completed_runs_contain_only_completed_or_skipped_steps`
- `failed_step_presence_requires_failure_count`
- `failed_run_has_failed_step_or_recorded_failure`

## Transitions
### `CreateRun`
- From: `Absent`
- On: `CreateRun`(step_ids)
- Guards:
  - `step_ids_are_non_empty`
- Emits: `EmitFlowRunNotice`
- To: `Pending`

### `StartRun`
- From: `Pending`
- On: `StartRun`()
- Emits: `EmitFlowRunNotice`
- To: `Running`

### `DispatchStep`
- From: `Running`
- On: `DispatchStep`(step_id)
- Guards:
  - `step_is_tracked`
  - `item_is_not_yet_dispatched`
- Emits: `EmitStepNotice`, `AdmitStepWork`
- To: `Running`

### `CompleteStep`
- From: `Running`
- On: `CompleteStep`(step_id)
- Guards:
  - `step_is_tracked`
  - `step_is_dispatched`
- Emits: `EmitStepNotice`
- To: `Running`

### `RecordStepOutput`
- From: `Running`
- On: `RecordStepOutput`(step_id)
- Guards:
  - `step_is_tracked`
  - `step_is_completed`
  - `output_not_yet_recorded`
- Emits: `PersistStepOutput`
- To: `Running`

### `FailStep`
- From: `Running`
- On: `FailStep`(step_id)
- Guards:
  - `step_is_tracked`
  - `step_is_dispatched`
- Emits: `EmitStepNotice`, `AppendFailureLedger`
- To: `Running`

### `SkipStep`
- From: `Running`
- On: `SkipStep`(step_id)
- Guards:
  - `step_is_tracked`
  - `step_is_not_started`
- Emits: `EmitStepNotice`
- To: `Running`

### `CancelStep`
- From: `Running`
- On: `CancelStep`(step_id)
- Guards:
  - `step_is_tracked`
  - `step_is_cancelable`
- Emits: `EmitStepNotice`
- To: `Running`

### `TerminalizeCompleted`
- From: `Running`
- On: `TerminalizeCompleted`()
- Guards:
  - `all_steps_are_completed_or_skipped`
- Emits: `EmitFlowRunNotice`, `FlowTerminalized`
- To: `Completed`

### `TerminalizeFailed`
- From: `Running`
- On: `TerminalizeFailed`()
- Guards:
  - `no_step_remains_dispatched`
  - `at_least_one_step_failed`
- Emits: `EmitFlowRunNotice`, `FlowTerminalized`
- To: `Failed`

### `TerminalizeCanceled`
- From: `Running`
- On: `TerminalizeCanceled`()
- Guards:
  - `no_step_remains_dispatched`
  - `at_least_one_step_canceled`
- Emits: `EmitFlowRunNotice`, `FlowTerminalized`
- To: `Canceled`

## Coverage
### Code Anchors
- `meerkat-mob/src/run.rs` — durable flow run aggregate precursor
- `meerkat-mob/src/runtime/flow.rs` — flow dispatch precursor
- `meerkat-mob/src/runtime/terminalization.rs` — CAS-guarded terminalization precursor

### Scenarios
- `create-dispatch-complete` — flow run creates, dispatches steps, and records completion
- `dependency-ready-evaluation` — dependency state drives ready-set and next-step admission
- `terminalize-on-failure-or-cancel` — failed or canceled runs terminalize deterministically
