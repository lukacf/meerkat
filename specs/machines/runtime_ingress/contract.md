# RuntimeIngressMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-runtime` / `machines::runtime_ingress`

## State
- Phase enum: `Active | Retired | Destroyed`
- `admitted_inputs`: `Set<InputId>`
- `admission_order`: `Seq<InputId>`
- `input_kind`: `Map<InputId, InputKind>`
- `policy_snapshot`: `Map<InputId, PolicyDecision>`
- `lifecycle`: `Map<InputId, InputLifecycleState>`
- `terminal_outcome`: `Map<InputId, Option<InputTerminalOutcome>>`
- `queue`: `Seq<InputId>`
- `current_run`: `Option<RunId>`
- `current_run_contributors`: `Seq<InputId>`
- `last_run`: `Map<InputId, Option<RunId>>`
- `last_boundary_sequence`: `Map<InputId, Option<BoundarySequence>>`
- `wake_requested`: `Bool`
- `process_requested`: `Bool`

## Inputs
- `AdmitQueued`(input_id: InputId, input_kind: InputKind, policy: PolicyDecision, wake: Bool, process: Bool)
- `AdmitConsumedOnAccept`(input_id: InputId, input_kind: InputKind, policy: PolicyDecision)
- `StageDrainSnapshot`(run_id: RunId, contributing_input_ids: Seq<InputId>)
- `BoundaryApplied`(run_id: RunId, boundary_sequence: u64)
- `RunCompleted`(run_id: RunId)
- `RunFailed`(run_id: RunId)
- `RunCancelled`(run_id: RunId)
- `SupersedeQueuedInput`(new_input_id: InputId, old_input_id: InputId)
- `CoalesceQueuedInputs`(aggregate_input_id: InputId, source_input_ids: Seq<InputId>)
- `Retire`
- `Reset`
- `Destroy`
- `Recover`

## Effects
- `IngressAccepted`(input_id: InputId)
- `ReadyForRun`(run_id: RunId, contributing_input_ids: Seq<InputId>)
- `InputLifecycleNotice`(input_id: InputId, new_state: InputLifecycleState)
- `WakeRuntime`
- `RequestImmediateProcessing`
- `CompletionResolved`(input_id: InputId, outcome: InputTerminalOutcome)
- `IngressNotice`(kind: String, detail: String)

## Invariants
- `queue_entries_are_queued`
- `terminal_inputs_do_not_appear_in_queue`
- `current_run_matches_contributor_presence`
- `staged_contributors_are_not_queued`
- `applied_pending_consumption_has_last_run`

## Transitions
### `AdmitQueuedNone`
- From: `Active`
- On: `AdmitQueued`(input_id, input_kind, policy, wake, process)
- Guards:
  - `input_is_new`
  - `wake_is_false`
  - `process_is_false`
- Emits: `IngressAccepted`, `InputLifecycleNotice`
- To: `Active`

### `AdmitQueuedWake`
- From: `Active`
- On: `AdmitQueued`(input_id, input_kind, policy, wake, process)
- Guards:
  - `input_is_new`
  - `wake_is_true`
  - `process_is_false`
- Emits: `IngressAccepted`, `InputLifecycleNotice`, `WakeRuntime`
- To: `Active`

### `AdmitQueuedProcess`
- From: `Active`
- On: `AdmitQueued`(input_id, input_kind, policy, wake, process)
- Guards:
  - `input_is_new`
  - `wake_is_false`
  - `process_is_true`
- Emits: `IngressAccepted`, `InputLifecycleNotice`, `RequestImmediateProcessing`
- To: `Active`

### `AdmitQueuedWakeAndProcess`
- From: `Active`
- On: `AdmitQueued`(input_id, input_kind, policy, wake, process)
- Guards:
  - `input_is_new`
  - `wake_is_true`
  - `process_is_true`
- Emits: `IngressAccepted`, `InputLifecycleNotice`, `WakeRuntime`, `RequestImmediateProcessing`
- To: `Active`

### `AdmitConsumedOnAccept`
- From: `Active`
- On: `AdmitConsumedOnAccept`(input_id, input_kind, policy)
- Guards:
  - `input_is_new`
- Emits: `IngressAccepted`, `InputLifecycleNotice`, `CompletionResolved`
- To: `Active`

### `StageDrainSnapshot`
- From: `Active`, `Retired`
- On: `StageDrainSnapshot`(run_id, contributing_input_ids)
- Guards:
  - `no_current_run`
  - `contributors_non_empty`
  - `contributors_are_queue_prefix`
  - `all_contributors_are_queued`
- Emits: `ReadyForRun`
- To: `Active`

### `BoundaryApplied`
- From: `Active`, `Retired`
- On: `BoundaryApplied`(run_id, boundary_sequence)
- Guards:
  - `run_matches_current`
  - `contributors_are_staged`
- Emits: `IngressNotice`
- To: `Active`

### `RunCompleted`
- From: `Active`, `Retired`
- On: `RunCompleted`(run_id)
- Guards:
  - `run_matches_current`
  - `contributors_pending_consumption`
- Emits: `IngressNotice`, `CompletionResolved`
- To: `Active`

### `RunFailed`
- From: `Active`, `Retired`
- On: `RunFailed`(run_id)
- Guards:
  - `run_matches_current`
  - `contributors_are_staged`
- Emits: `IngressNotice`
- To: `Active`

### `RunCancelled`
- From: `Active`, `Retired`
- On: `RunCancelled`(run_id)
- Guards:
  - `run_matches_current`
  - `contributors_are_staged`
- Emits: `IngressNotice`
- To: `Active`

### `SupersedeQueuedInput`
- From: `Active`
- On: `SupersedeQueuedInput`(new_input_id, old_input_id)
- Guards:
  - `new_input_is_admitted`
  - `old_input_is_queued`
- Emits: `IngressNotice`
- To: `Active`

### `CoalesceQueuedInputs`
- From: `Active`
- On: `CoalesceQueuedInputs`(aggregate_input_id, source_input_ids)
- Guards:
  - `aggregate_input_is_admitted`
  - `sources_non_empty`
  - `all_sources_are_queued`
- Emits: `IngressNotice`
- To: `Active`

### `Retire`
- From: `Active`
- On: `Retire`()
- Emits: `IngressNotice`
- To: `Retired`

### `ResetFromActive`
- From: `Active`
- On: `Reset`()
- Emits: `IngressNotice`
- To: `Active`

### `ResetFromRetired`
- From: `Retired`
- On: `Reset`()
- Emits: `IngressNotice`
- To: `Active`

### `Destroy`
- From: `Active`, `Retired`
- On: `Destroy`()
- Emits: `IngressNotice`
- To: `Destroyed`

### `RecoverFromActive`
- From: `Active`
- On: `Recover`()
- Emits: `IngressNotice`
- To: `Active`

### `RecoverFromRetired`
- From: `Retired`
- On: `Recover`()
- Emits: `IngressNotice`
- To: `Retired`

## Coverage
### Code Anchors
- `meerkat-runtime/src/input.rs` — runtime ingress input taxonomy precursor
- `meerkat-runtime/src/queue.rs` — ordered queue discipline precursor
- `meerkat-runtime/src/driver/ephemeral.rs` — ephemeral ingress mutation precursor
- `meerkat-runtime/src/driver/persistent.rs` — persistent ingress/recovery precursor

### Scenarios
- `admit-and-stage-prefix` — individually admitted inputs form a runtime-authored staged prefix
- `prompt-queue` — queued user prompt enters ingress without immediate processing when already running
- `prompt-steer` — steering user prompt enters ingress with immediate-processing intent
- `rollback-on-failure` — failed or cancelled run restores staged contributors to the queue front
- `recover-retire-reset-destroy` — recovery and lifecycle terminalization preserve contributor legality
