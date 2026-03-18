# InputLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-runtime` / `generated::input_lifecycle`

## State
- Phase enum: `Accepted | Queued | Staged | Applied | AppliedPendingConsumption | Consumed | Superseded | Coalesced | Abandoned`
- `terminal_outcome`: `Option<InputTerminalOutcome>`
- `last_run_id`: `Option<RunId>`
- `last_boundary_sequence`: `Option<BoundarySequence>`

## Inputs
- `QueueAccepted`
- `StageForRun`(run_id: RunId)
- `RollbackStaged`
- `MarkApplied`(run_id: RunId)
- `MarkAppliedPendingConsumption`(boundary_sequence: BoundarySequence)
- `Consume`
- `Supersede`
- `Coalesce`
- `Abandon`

## Effects
- `InputLifecycleNotice`(new_state: InputLifecycleState)
- `RecordTerminalOutcome`(outcome: InputTerminalOutcome)
- `RecordRunAssociation`(run_id: RunId)
- `RecordBoundarySequence`(boundary_sequence: BoundarySequence)

## Invariants
- `accepted_has_no_run_or_boundary_metadata`
- `boundary_metadata_requires_application`

## Transitions
### `QueueAccepted`
- From: `Accepted`
- On: `QueueAccepted`()
- Emits: `InputLifecycleNotice`
- To: `Queued`

### `StageForRun`
- From: `Queued`
- On: `StageForRun`(run_id)
- Emits: `InputLifecycleNotice`, `RecordRunAssociation`
- To: `Staged`

### `RollbackStaged`
- From: `Staged`
- On: `RollbackStaged`()
- Emits: `InputLifecycleNotice`
- To: `Queued`

### `MarkApplied`
- From: `Staged`
- On: `MarkApplied`(run_id)
- Guards:
  - `matches_last_run`
- Emits: `InputLifecycleNotice`
- To: `Applied`

### `MarkAppliedPendingConsumption`
- From: `Applied`
- On: `MarkAppliedPendingConsumption`(boundary_sequence)
- Emits: `InputLifecycleNotice`, `RecordBoundarySequence`
- To: `AppliedPendingConsumption`

### `Consume`
- From: `AppliedPendingConsumption`
- On: `Consume`()
- Emits: `InputLifecycleNotice`, `RecordTerminalOutcome`
- To: `Consumed`

### `Supersede`
- From: `Queued`
- On: `Supersede`()
- Emits: `InputLifecycleNotice`, `RecordTerminalOutcome`
- To: `Superseded`

### `Coalesce`
- From: `Queued`
- On: `Coalesce`()
- Emits: `InputLifecycleNotice`, `RecordTerminalOutcome`
- To: `Coalesced`

### `Abandon`
- From: `Accepted`, `Queued`, `Staged`, `Applied`, `AppliedPendingConsumption`
- On: `Abandon`()
- Emits: `InputLifecycleNotice`, `RecordTerminalOutcome`
- To: `Abandoned`

## Coverage
### Code Anchors
- `meerkat-runtime/src/input_state.rs` — authoritative input lifecycle record shape
- `meerkat-runtime/src/input_machine.rs` — lifecycle transition validator/reducer precursor
- `meerkat-runtime/src/input_ledger.rs` — runtime-owned lifecycle ledger precursor

### Scenarios
- `queue-stage-apply-consume` — accepted input queues, stages, applies, and is consumed at a boundary
- `supersede-coalesce` — queued input is terminalized by supersession or coalescing
- `abandon` — input is abandoned cleanly during reset/destroy style terminalization
