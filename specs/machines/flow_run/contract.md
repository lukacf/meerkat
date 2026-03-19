# FlowRunMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-mob` / `generated::flow_run`

## State
- Phase enum: `Absent | Pending | Running | Completed | Failed | Canceled`
- `tracked_steps`: `Set<StepId>`
- `ordered_steps`: `Seq<StepId>`
- `step_status`: `Map<StepId, Option<StepRunStatus>>`
- `output_recorded`: `Map<StepId, Bool>`
- `step_condition_results`: `Map<StepId, Option<Bool>>`
- `step_has_conditions`: `Map<StepId, Bool>`
- `step_dependencies`: `Map<StepId, Seq<StepId>>`
- `step_dependency_modes`: `Map<StepId, DependencyMode>`
- `step_branches`: `Map<StepId, Option<BranchId>>`
- `step_collection_policies`: `Map<StepId, CollectionPolicyKind>`
- `step_quorum_thresholds`: `Map<StepId, u32>`
- `step_target_counts`: `Map<StepId, u32>`
- `step_target_success_counts`: `Map<StepId, u32>`
- `step_target_terminal_failure_counts`: `Map<StepId, u32>`
- `target_retry_counts`: `Map<String, u32>`
- `failure_count`: `u32`
- `consecutive_failure_count`: `u32`
- `escalation_threshold`: `u32`
- `max_step_retries`: `u32`

## Inputs
- `CreateRun`(step_ids: Seq<StepId>, ordered_steps: Seq<StepId>, step_has_conditions: Map<StepId, Bool>, step_dependencies: Map<StepId, Seq<StepId>>, step_dependency_modes: Map<StepId, DependencyMode>, step_branches: Map<StepId, Option<BranchId>>, step_collection_policies: Map<StepId, CollectionPolicyKind>, step_quorum_thresholds: Map<StepId, u32>, escalation_threshold: u32, max_step_retries: u32)
- `StartRun`
- `DispatchStep`(step_id: StepId)
- `CompleteStep`(step_id: StepId)
- `RecordStepOutput`(step_id: StepId)
- `ConditionPassed`(step_id: StepId)
- `ConditionRejected`(step_id: StepId)
- `FailStep`(step_id: StepId)
- `SkipStep`(step_id: StepId)
- `CancelStep`(step_id: StepId)
- `RegisterTargets`(step_id: StepId, target_count: u32)
- `RecordTargetSuccess`(step_id: StepId, target_id: MeerkatId)
- `RecordTargetTerminalFailure`(step_id: StepId)
- `RecordTargetCanceled`(step_id: StepId, target_id: MeerkatId)
- `RecordTargetFailure`(step_id: StepId, target_id: MeerkatId, retry_key: String)
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
- `EscalateSupervisor`(step_id: StepId)
- `ProjectTargetSuccess`(step_id: StepId, target_id: MeerkatId)
- `ProjectTargetFailure`(step_id: StepId, target_id: MeerkatId)
- `ProjectTargetCanceled`(step_id: StepId, target_id: MeerkatId)

## Helpers
- `RunIsTerminal`() -> `Bool`
- `StepIsTracked`(step_id: StepId) -> `Bool`
- `StepStatusIs`(step_id: StepId, expected_status: StepRunStatus) -> `Bool`
- `StepOutputRecordedIs`(step_id: StepId, expected: Bool) -> `Bool`
- `StepConditionRecordedIs`(step_id: StepId, expected: Option<Bool>) -> `Bool`
- `StepConditionAllowsDispatch`(step_id: StepId) -> `Bool`
- `AllTrackedStepsInAllowedStatuses`(allowed_statuses: Seq<Option<StepRunStatus>>) -> `Bool`
- `NoTrackedStepInStatus`(status: StepRunStatus) -> `Bool`
- `AnyTrackedStepInStatus`(status: StepRunStatus) -> `Bool`
- `StepHasDependencies`(step_id: StepId) -> `Bool`
- `AllDependenciesCompleted`(step_id: StepId) -> `Bool`
- `AllDependenciesSkipped`(step_id: StepId) -> `Bool`
- `AnyDependencyCompleted`(step_id: StepId) -> `Bool`
- `StepDependencyReady`(step_id: StepId) -> `Bool`
- `StepDependencyShouldSkip`(step_id: StepId) -> `Bool`
- `StepBranchBlocked`(step_id: StepId) -> `Bool`
- `EscalationWillTrigger`() -> `Bool`
- `TargetRetryCount`(retry_key: String) -> `u32`
- `TargetRetryAllowed`(retry_key: String) -> `Bool`
- `CollectionSatisfied`(step_id: StepId) -> `Bool`
- `CollectionFeasible`(step_id: StepId) -> `Bool`
- `StepTargetCount`(step_id: StepId) -> `u32`
- `StepTargetSuccessCount`(step_id: StepId) -> `u32`
- `StepTargetTerminalFailureCount`(step_id: StepId) -> `u32`
- `RemainingTargetCount`(step_id: StepId) -> `u32`

## Invariants
- `output_only_follows_completed_steps`
- `terminal_runs_have_no_dispatched_steps`
- `completed_runs_contain_only_completed_or_skipped_steps`
- `failed_step_presence_requires_failure_count`
- `failed_run_has_failed_step_or_recorded_failure`

## Transitions
### `CreateRun`
- From: `Absent`
- On: `CreateRun`(step_ids, ordered_steps, step_has_conditions, step_dependencies, step_dependency_modes, step_branches, step_collection_policies, step_quorum_thresholds, escalation_threshold, max_step_retries)
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
  - `condition_allows_dispatch`
  - `dependencies_are_ready`
  - `branch_is_not_blocked`
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

### `ConditionPassed`
- From: `Running`
- On: `ConditionPassed`(step_id)
- Guards:
  - `step_is_tracked`
  - `step_is_not_started`
- To: `Running`

### `ConditionRejected`
- From: `Running`
- On: `ConditionRejected`(step_id)
- Guards:
  - `step_is_tracked`
  - `step_is_not_started`
- Emits: `EmitStepNotice`
- To: `Running`

### `FailStepEscalating`
- From: `Running`
- On: `FailStep`(step_id)
- Guards:
  - `step_is_tracked`
  - `step_is_dispatched`
  - `escalation_will_trigger`
- Emits: `EmitStepNotice`, `AppendFailureLedger`, `EscalateSupervisor`
- To: `Running`

### `FailStep`
- From: `Running`
- On: `FailStep`(step_id)
- Guards:
  - `step_is_tracked`
  - `step_is_dispatched`
  - `escalation_does_not_trigger`
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

### `RegisterTargets`
- From: `Running`
- On: `RegisterTargets`(step_id, target_count)
- Guards:
  - `step_is_tracked`
  - `step_is_not_started`
- To: `Running`

### `RecordTargetSuccess`
- From: `Running`
- On: `RecordTargetSuccess`(step_id, target_id)
- Guards:
  - `step_is_tracked`
  - `step_is_dispatched`
- Emits: `ProjectTargetSuccess`
- To: `Running`

### `RecordTargetTerminalFailure`
- From: `Running`
- On: `RecordTargetTerminalFailure`(step_id)
- Guards:
  - `step_is_tracked`
  - `step_is_dispatched`
- To: `Running`

### `RecordTargetCanceled`
- From: `Running`
- On: `RecordTargetCanceled`(step_id, target_id)
- Guards:
  - `step_is_tracked`
  - `step_is_dispatched`
- Emits: `ProjectTargetCanceled`
- To: `Running`

### `RecordTargetFailure`
- From: `Running`
- On: `RecordTargetFailure`(step_id, target_id, retry_key)
- Guards:
  - `step_is_tracked`
  - `step_is_dispatched`
- Emits: `ProjectTargetFailure`, `AppendFailureLedger`
- To: `Running`

### `TerminalizeCompleted`
- From: `Running`
- On: `TerminalizeCompleted`()
- Guards:
  - `all_steps_are_completed_or_skipped`
- Emits: `EmitFlowRunNotice`, `FlowTerminalized`
- To: `Completed`

### `TerminalizeFailed`
- From: `Pending`, `Running`
- On: `TerminalizeFailed`()
- Guards:
  - `no_step_remains_dispatched`
- Emits: `EmitFlowRunNotice`, `FlowTerminalized`
- To: `Failed`

### `TerminalizeCanceled`
- From: `Pending`, `Running`
- On: `TerminalizeCanceled`()
- Guards:
  - `no_step_remains_dispatched`
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
