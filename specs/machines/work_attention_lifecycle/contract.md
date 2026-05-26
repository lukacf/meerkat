# WorkAttentionLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::work_attention_lifecycle`

## State
- Phase enum: `Active | Paused | Superseded | Stopped`
- `revision`: `u64`
- `paused_until_utc_ms`: `Option<u64>`
- `superseded_by_binding_key`: `Option<WorkAttentionBindingKey>`
- `terminal_at_utc_ms`: `Option<u64>`

## Inputs
- `Pause`(expected_revision: u64, until_utc_ms: Option<u64>)
- `Resume`(expected_revision: u64)
- `Supersede`(expected_revision: u64, superseded_by_binding_key: WorkAttentionBindingKey, at_utc_ms: u64)
- `Stop`(expected_revision: u64, at_utc_ms: u64)

## Signals

## Effects
- `AttentionPaused`(revision: u64)
- `AttentionResumed`(revision: u64)
- `AttentionSuperseded`(revision: u64)
- `AttentionStopped`(revision: u64)

## Invariants
- `live_has_no_terminal_time`
- `paused_has_pause_state`
- `superseded_records_successor`

## Transitions
### `PauseActive`
- From: `Active`
- On: `Pause`(expected_revision, until_utc_ms)
- Guards:
  - ``
- Emits: `AttentionPaused`
- To: `Paused`

### `PausePaused`
- From: `Paused`
- On: `Pause`(expected_revision, until_utc_ms)
- Guards:
  - ``
- Emits: `AttentionPaused`
- To: `Paused`

### `ResumePaused`
- From: `Paused`
- On: `Resume`(expected_revision)
- Guards:
  - ``
- Emits: `AttentionResumed`
- To: `Active`

### `SupersedeActive`
- From: `Active`
- On: `Supersede`(expected_revision, superseded_by_binding_key, at_utc_ms)
- Guards:
  - ``
- Emits: `AttentionSuperseded`
- To: `Superseded`

### `SupersedePaused`
- From: `Paused`
- On: `Supersede`(expected_revision, superseded_by_binding_key, at_utc_ms)
- Guards:
  - ``
- Emits: `AttentionSuperseded`
- To: `Superseded`

### `StopActive`
- From: `Active`
- On: `Stop`(expected_revision, at_utc_ms)
- Guards:
  - ``
- Emits: `AttentionStopped`
- To: `Stopped`

### `StopPaused`
- From: `Paused`
- On: `Stop`(expected_revision, at_utc_ms)
- Guards:
  - ``
- Emits: `AttentionStopped`
- To: `Stopped`

## Coverage
### Code Anchors
- `meerkat-workgraph/src/machine.rs` — WorkAttentionMachine domain-facing lifecycle transition seam over Pause, Resume, Stop, and Supersede; effects Paused, Resumed, Stopped, Superseded; invariants active_has_no_pause_deadline, paused_has_pause_deadline, stopped_has_stop_time, superseded_has_target; revision, timed pause eligibility, stopped state, and supersession target ownership

### Scenarios
- `work_attention_pause_resume_stop` — PauseActive, PausePaused, ResumePaused, SupersedeActive, SupersedePaused, StopActive, StopPaused, AttentionPaused, AttentionResumed, AttentionSuperseded, AttentionStopped, live_has_no_terminal_time, paused_has_pause_state, superseded_records_successor, timed pause eligibility, CAS revision, and terminal work item attention stop stay under WorkAttentionLifecycleMachine authority
