# ScheduleLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `meerkat-schedule` / `generated::schedule_lifecycle`

## State
- Phase enum: `Active | Paused | Deleted`
- `revision`: `u64`
- `trigger_key`: `String`
- `target_binding_key`: `String`
- `misfire_policy`: `MisfirePolicy`
- `overlap_policy`: `OverlapPolicy`
- `missing_target_policy`: `MissingTargetPolicy`
- `planning_cursor_utc_ms`: `Option<u64>`
- `next_occurrence_ordinal`: `u64`

## Inputs
- `Revise`(trigger_key: String, target_binding_key: String, misfire_policy: MisfirePolicy, overlap_policy: OverlapPolicy, missing_target_policy: MissingTargetPolicy)
- `RecordPlanningWindow`(planning_cursor_utc_ms: u64, next_occurrence_ordinal: u64)
- `Pause`
- `Resume`
- `Delete`

## Signals

## Effects
- `EmitScheduleNotice`(new_state: ScheduleLifecycleState, revision: u64)
- `SupersedePendingOccurrences`(superseding_revision: u64)
- `PlanningWindowRecorded`(planning_cursor_utc_ms: u64, next_occurrence_ordinal: u64)

## Invariants
- `revision_is_positive`
- `deleted_has_no_planning_cursor`
- `planning_cursor_requires_occurrence_progress`

## Transitions
### `ReviseActive`
- From: `Active`
- On: `Revise`(trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy)
- Emits: `EmitScheduleNotice`, `SupersedePendingOccurrences`
- To: `Active`

### `RevisePaused`
- From: `Paused`
- On: `Revise`(trigger_key, target_binding_key, misfire_policy, overlap_policy, missing_target_policy)
- Emits: `EmitScheduleNotice`, `SupersedePendingOccurrences`
- To: `Paused`

### `RecordPlanningWindowActive`
- From: `Active`
- On: `RecordPlanningWindow`(planning_cursor_utc_ms, next_occurrence_ordinal)
- Guards:
  - `planning_window_advances_ordinal`
- Emits: `EmitScheduleNotice`, `PlanningWindowRecorded`
- To: `Active`

### `RecordPlanningWindowPaused`
- From: `Paused`
- On: `RecordPlanningWindow`(planning_cursor_utc_ms, next_occurrence_ordinal)
- Guards:
  - `planning_window_advances_ordinal`
- Emits: `EmitScheduleNotice`, `PlanningWindowRecorded`
- To: `Paused`

### `PauseActive`
- From: `Active`
- On: `Pause`()
- Emits: `EmitScheduleNotice`
- To: `Paused`

### `ResumePaused`
- From: `Paused`
- On: `Resume`()
- Emits: `EmitScheduleNotice`
- To: `Active`

### `DeleteActive`
- From: `Active`
- On: `Delete`()
- Emits: `EmitScheduleNotice`, `SupersedePendingOccurrences`
- To: `Deleted`

### `DeletePaused`
- From: `Paused`
- On: `Delete`()
- Emits: `EmitScheduleNotice`, `SupersedePendingOccurrences`
- To: `Deleted`

## Coverage
### Code Anchors
- `meerkat-schedule/src/authority.rs` — schedule lifecycle authority and revision ownership

### Scenarios
- `schedule_pause_resume_delete` — schedule transitions through create, pause, resume, and delete while advancing revision
