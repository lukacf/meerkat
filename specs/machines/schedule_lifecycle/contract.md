# ScheduleLifecycleMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `1`
- Rust owner: `self` / `catalog::dsl::schedule_lifecycle`

## State
- Phase enum: `Active | Paused | Deleted`
- `schedule_id`: `ScheduleId`
- `revision`: `u64`
- `trigger_key`: `String`
- `target_binding_key`: `String`
- `misfire_policy`: `MisfirePolicy`
- `misfire_policy_key`: `String`
- `overlap_policy`: `OverlapPolicy`
- `overlap_policy_key`: `String`
- `missing_target_policy`: `MissingTargetPolicy`
- `missing_target_policy_key`: `String`
- `planning_horizon_days`: `u64`
- `planning_horizon_occurrences`: `u64`
- `planning_cursor_utc_ms`: `Option<u64>`
- `next_occurrence_ordinal`: `u64`
- `superseded_ack_ids`: `Set<OccurrenceId>`

## Inputs
- `Create`(schedule_id: ScheduleId, trigger_key: String, target_binding_key: String, misfire_policy: MisfirePolicy, misfire_policy_key: String, overlap_policy: OverlapPolicy, overlap_policy_key: String, missing_target_policy: MissingTargetPolicy, missing_target_policy_key: String, planning_horizon_days: Option<u64>, planning_horizon_occurrences: Option<u64>)
- `Revise`(trigger_key: String, target_binding_key: String, misfire_policy: MisfirePolicy, misfire_policy_key: String, overlap_policy: OverlapPolicy, overlap_policy_key: String, missing_target_policy: MissingTargetPolicy, missing_target_policy_key: String, planning_horizon_days: u64, planning_horizon_occurrences: u64, at_utc_ms: u64)
- `UpdatePlanningConfig`(planning_horizon_days: u64, planning_horizon_occurrences: u64)
- `RecordPlanningWindow`(planning_cursor_utc_ms: u64, next_occurrence_ordinal: u64)
- `SyncTargetSnapshot`(target_binding_key: String)
- `Pause`(at_utc_ms: u64)
- `Resume`(at_utc_ms: u64)
- `Delete`(at_utc_ms: u64)
- `ConfirmOccurrencesSuperseded`(occurrence_id: OccurrenceId, superseding_revision: u64)

## Signals

## Effects
- `EmitScheduleNotice`(new_state: ScheduleLifecycleState, revision: u64)
- `SupersedePendingOccurrences`(superseding_revision: u64, at_utc_ms: u64)
- `PlanningWindowRecorded`(planning_cursor_utc_ms: u64, next_occurrence_ordinal: u64)

## Invariants
- `revision_is_positive`
- `deleted_has_no_planning_cursor`
- `planning_cursor_requires_occurrence_progress`

## Transitions
### `CreateSchedule`
- From: `Active`
- On: `Create`(schedule_id, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences)
- Emits: `EmitScheduleNotice`
- To: `Active`

### `ReviseActive`
- From: `Active`
- On: `Revise`(trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, at_utc_ms)
- Emits: `EmitScheduleNotice`, `SupersedePendingOccurrences`
- To: `Active`

### `RevisePaused`
- From: `Paused`
- On: `Revise`(trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, planning_horizon_days, planning_horizon_occurrences, at_utc_ms)
- Emits: `EmitScheduleNotice`, `SupersedePendingOccurrences`
- To: `Paused`

### `UpdatePlanningConfigActive`
- From: `Active`
- On: `UpdatePlanningConfig`(planning_horizon_days, planning_horizon_occurrences)
- Emits: `EmitScheduleNotice`
- To: `Active`

### `UpdatePlanningConfigPaused`
- From: `Paused`
- On: `UpdatePlanningConfig`(planning_horizon_days, planning_horizon_occurrences)
- Emits: `EmitScheduleNotice`
- To: `Paused`

### `RecordPlanningWindowActive`
- From: `Active`
- On: `RecordPlanningWindow`(planning_cursor_utc_ms, next_occurrence_ordinal)
- Guards:
  - `planning_window_advances_ordinal`
- Emits: `EmitScheduleNotice`, `PlanningWindowRecorded`
- To: `Active`

### `SyncTargetSnapshotActive`
- From: `Active`
- On: `SyncTargetSnapshot`(target_binding_key)
- To: `Active`

### `SyncTargetSnapshotPaused`
- From: `Paused`
- On: `SyncTargetSnapshot`(target_binding_key)
- To: `Paused`

### `PauseActiveOrPaused`
- From: `Active`, `Paused`
- On: `Pause`(at_utc_ms)
- Emits: `EmitScheduleNotice`
- To: `Paused`

### `ResumeActiveOrPaused`
- From: `Active`, `Paused`
- On: `Resume`(at_utc_ms)
- Emits: `EmitScheduleNotice`
- To: `Active`

### `DeleteActive`
- From: `Active`
- On: `Delete`(at_utc_ms)
- Emits: `EmitScheduleNotice`, `SupersedePendingOccurrences`
- To: `Deleted`

### `DeletePaused`
- From: `Paused`
- On: `Delete`(at_utc_ms)
- Emits: `EmitScheduleNotice`, `SupersedePendingOccurrences`
- To: `Deleted`

### `DeleteDeleted`
- From: `Deleted`
- On: `Delete`(at_utc_ms)
- To: `Deleted`

### `ConfirmOccurrencesSupersededActive`
- From: `Active`
- On: `ConfirmOccurrencesSuperseded`(occurrence_id, superseding_revision)
- To: `Active`

### `ConfirmOccurrencesSupersededPaused`
- From: `Paused`
- On: `ConfirmOccurrencesSuperseded`(occurrence_id, superseding_revision)
- To: `Paused`

### `ConfirmOccurrencesSupersededDeleted`
- From: `Deleted`
- On: `ConfirmOccurrencesSuperseded`(occurrence_id, superseding_revision)
- To: `Deleted`

## Coverage
### Code Anchors
- `schedule_lifecycle` (machine `ScheduleLifecycleMachine`): `meerkat-schedule/src/lifecycle.rs` — Schedule::apply domain-facing lifecycle transition seam over create, revise, update planning config active or paused, planning window, pause, resume, delete, supersede pending occurrences, sync target snapshot for active or paused materialized session bindings, revision, and planning cursor rules

### Scenarios
- `schedule_pause_resume_delete` — schedule transitions through create, pause, resume, and delete while advancing revision
- `schedule_revision_and_planning` — active or paused schedules revise, update planning config active or paused, record planning windows, sync target snapshots for materialized session bindings, confirm superseded occurrences, supersede pending occurrences, maintain positive revision, and require occurrence progress for planning cursor
