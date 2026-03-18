# RuntimeIngressMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `2`
- Rust owner: `meerkat-runtime` / `generated::runtime_ingress`

## State
- Phase enum: `Active | Retired | Destroyed`
- `admitted_inputs`: `Set<WorkId>`
- `admission_order`: `Seq<WorkId>`
- `content_shape`: `Map<WorkId, ContentShape>`
- `request_id`: `Map<WorkId, Option<RequestId>>`
- `reservation_key`: `Map<WorkId, Option<ReservationKey>>`
- `policy_snapshot`: `Map<WorkId, PolicyDecision>`
- `handling_mode`: `Map<WorkId, HandlingMode>`
- `lifecycle`: `Map<WorkId, InputLifecycleState>`
- `terminal_outcome`: `Map<WorkId, Option<InputTerminalOutcome>>`
- `queue`: `Seq<WorkId>`
- `steer_queue`: `Seq<WorkId>`
- `current_run`: `Option<RunId>`
- `current_run_contributors`: `Seq<WorkId>`
- `last_run`: `Map<WorkId, Option<RunId>>`
- `last_boundary_sequence`: `Map<WorkId, Option<BoundarySequence>>`
- `wake_requested`: `Bool`
- `process_requested`: `Bool`
- `silent_intent_overrides`: `Set<String>`

## Inputs
- `AdmitQueued`(work_id: WorkId, content_shape: ContentShape, handling_mode: HandlingMode, request_id: Option<RequestId>, reservation_key: Option<ReservationKey>, policy: PolicyDecision)
- `AdmitConsumedOnAccept`(work_id: WorkId, content_shape: ContentShape, request_id: Option<RequestId>, reservation_key: Option<ReservationKey>, policy: PolicyDecision)
- `StageDrainSnapshot`(run_id: RunId, contributing_work_ids: Seq<WorkId>)
- `BoundaryApplied`(run_id: RunId, boundary_sequence: u64)
- `RunCompleted`(run_id: RunId)
- `RunFailed`(run_id: RunId)
- `RunCancelled`(run_id: RunId)
- `SupersedeQueuedInput`(new_work_id: WorkId, old_work_id: WorkId)
- `CoalesceQueuedInputs`(aggregate_work_id: WorkId, source_work_ids: Seq<WorkId>)
- `Retire`
- `Reset`
- `Destroy`
- `Recover`
- `SetSilentIntentOverrides`(intents: Set<String>)

## Effects
- `IngressAccepted`(work_id: WorkId)
- `ReadyForRun`(run_id: RunId, contributing_work_ids: Seq<WorkId>)
- `InputLifecycleNotice`(work_id: WorkId, new_state: InputLifecycleState)
- `WakeRuntime`
- `RequestImmediateProcessing`
- `CompletionResolved`(work_id: WorkId, outcome: InputTerminalOutcome)
- `IngressNotice`(kind: String, detail: String)
- `SilentIntentApplied`(work_id: WorkId, intent: String)

## Invariants
- `queue_entries_are_queued`
- `steer_entries_are_queued`
- `pending_inputs_preserve_content_shape`
- `admitted_inputs_preserve_correlation_slots`
- `queue_entries_preserve_handling_mode`
- `steer_entries_preserve_handling_mode`
- `pending_queues_do_not_overlap`
- `terminal_inputs_do_not_appear_in_queue`
- `current_run_matches_contributor_presence`
- `staged_contributors_are_not_queued`
- `applied_pending_consumption_has_last_run`

## Transitions
### `AdmitQueuedQueue`
- From: `Active`
- On: `AdmitQueued`(work_id, content_shape, handling_mode, request_id, reservation_key, policy)
- Guards:
  - `input_is_new`
  - `handling_mode_is_queue`
- Emits: `IngressAccepted`, `InputLifecycleNotice`
- To: `Active`

### `AdmitQueuedSteer`
- From: `Active`
- On: `AdmitQueued`(work_id, content_shape, handling_mode, request_id, reservation_key, policy)
- Guards:
  - `input_is_new`
  - `handling_mode_is_steer`
- Emits: `IngressAccepted`, `InputLifecycleNotice`, `WakeRuntime`, `RequestImmediateProcessing`
- To: `Active`

### `AdmitConsumedOnAccept`
- From: `Active`
- On: `AdmitConsumedOnAccept`(work_id, content_shape, request_id, reservation_key, policy)
- Guards:
  - `input_is_new`
- Emits: `IngressAccepted`, `InputLifecycleNotice`, `CompletionResolved`
- To: `Active`

### `StageDrainSnapshot`
- From: `Active`, `Retired`
- On: `StageDrainSnapshot`(run_id, contributing_work_ids)
- Guards:
  - `no_current_run`
  - `contributors_non_empty`
  - `contributors_match_current_drain_source`
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
- On: `SupersedeQueuedInput`(new_work_id, old_work_id)
- Guards:
  - `new_input_is_admitted`
  - `old_input_is_queued`
- Emits: `IngressNotice`
- To: `Active`

### `CoalesceQueuedInputs`
- From: `Active`
- On: `CoalesceQueuedInputs`(aggregate_work_id, source_work_ids)
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

### `SetSilentIntentOverridesFromActive`
- From: `Active`
- On: `SetSilentIntentOverrides`(intents)
- Emits: `IngressNotice`
- To: `Active`

### `SetSilentIntentOverridesFromRetired`
- From: `Retired`
- On: `SetSilentIntentOverrides`(intents)
- Emits: `IngressNotice`
- To: `Retired`

## Coverage
### Code Anchors
- `meerkat-runtime/src/input.rs` — runtime ingress input taxonomy precursor
- `meerkat-runtime/src/queue.rs` — ordered queue discipline precursor
- `meerkat-runtime/src/driver/ephemeral.rs` — ephemeral ingress mutation precursor
- `meerkat-runtime/src/driver/persistent.rs` — persistent ingress/recovery precursor
- `meerkat-runtime/src/runtime_loop.rs` — same-boundary contributor batching and staged run precursor

### Scenarios
- `admit-and-stage-prefix` — individually admitted inputs form a runtime-authored staged prefix
- `prompt-queue` — queued user prompt enters ingress without immediate processing when already running
- `prompt-steer` — steering user prompt enters ingress with immediate-processing intent
- `rollback-on-failure` — failed or cancelled run restores staged contributors to the queue front
- `recover-retire-reset-destroy` — recovery and lifecycle terminalization preserve contributor legality
