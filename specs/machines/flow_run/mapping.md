# FlowRunMachine Mapping Note

This note maps the normative `0.5` `FlowRunMachine` contract onto current
`0.4` anchors.

## Rust anchors

- durable run aggregate:
  - `meerkat-mob/src/run.rs`
- store and CAS semantics:
  - `meerkat-mob/src/store/mod.rs`
  - `meerkat-mob/src/store/in_memory.rs`
- runtime execution/terminalization:
  - `meerkat-mob/src/runtime/flow.rs`
  - `meerkat-mob/src/runtime/terminalization.rs`

## What is already aligned

- `MobRunStatus` already exists as `Pending`, `Running`, `Completed`,
  `Failed`, `Canceled`
- step ledgers and failure ledgers already exist
- output persistence is already step-ledger-aware
- terminalization is already CAS-guarded and terminal-state-aware

## What the formal model abstracts

The TLA+ model deliberately abstracts away:

- concrete condition-expression evaluation mechanics
- template rendering and shared-path string interpolation mechanics
- topology lookup and target profile/member discovery mechanics
- concrete event streams, JSON payloads, and external executor timing

Those arrive at the machine boundary as typed inputs or effects.

The formal model does **not** abstract away the semantic truths that drive
flow progression. It explicitly owns:

- dependency readiness and dependency-skip truth
- branch-blocking / branch-winner truth
- collection policy satisfaction and feasibility
- retry-count progression and retry-allowed truth
- escalation-trigger truth

## Important `0.5` clarification

`FlowRunMachine` is the durable run owner. It is not a mob-local turn executor.

Step dispatch may produce runtime/turn work, but that work crosses machine
boundaries and returns through effects/terminalization rather than making flow
execution itself into a second execution loop.

## Known `0.4` divergence

- actor-side task/cancel trackers still coexist with store-owned durable truth
- condition evaluation, template rendering, schema validation, topology lookup,
  and executor outcomes remain external boundaries that feed typed facts into
  the machine

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `FlowRunMachine`

### Code Anchors
- `flow_run_aggregate`: `meerkat-mob/src/run.rs` — durable flow run aggregate precursor
- `flow_runtime`: `meerkat-mob/src/runtime/flow.rs` — flow dispatch precursor
- `flow_terminalization`: `meerkat-mob/src/runtime/terminalization.rs` — CAS-guarded terminalization precursor

### Scenarios
- `create-dispatch-complete` — flow run creates, dispatches steps, and records completion
- `dependency-ready-evaluation` — dependency state drives ready-set and next-step admission
- `terminalize-on-failure-or-cancel` — failed or canceled runs terminalize deterministically

### Transitions
- `CreateRun`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `StartRun`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `DispatchStep`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `CompleteStep`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `RecordStepOutput`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `ConditionPassed`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `ConditionRejected`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `FailStepEscalating`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `FailStep`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `SkipStep`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `CancelStep`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `RegisterTargets`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `RecordTargetSuccess`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `RecordTargetTerminalFailure`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `RecordTargetCanceled`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `RecordTargetFailure`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `TerminalizeCompleted`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `TerminalizeFailed`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `TerminalizeCanceled`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`

### Effects
- `EmitFlowRunNotice`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `EmitStepNotice`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `AppendFailureLedger`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `PersistStepOutput`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `AdmitStepWork`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `FlowTerminalized`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `EscalateSupervisor`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `ProjectTargetSuccess`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `ProjectTargetFailure`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `ProjectTargetCanceled`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`

### Invariants
- `output_only_follows_completed_steps`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `terminal_runs_have_no_dispatched_steps`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `create-dispatch-complete`
- `completed_runs_contain_only_completed_or_skipped_steps`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `failed_step_presence_requires_failure_count`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`
- `failed_run_has_failed_step_or_recorded_failure`
  - anchors: `flow_run_aggregate`, `flow_runtime`, `flow_terminalization`
  - scenarios: `terminalize-on-failure-or-cancel`


<!-- GENERATED_COVERAGE_END -->
