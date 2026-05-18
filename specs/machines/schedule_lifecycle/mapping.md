# ScheduleLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `ScheduleLifecycleMachine`

### Code Anchors
- `schedule_lifecycle`: `meerkat-schedule/src/lifecycle.rs` — Schedule::apply domain-facing lifecycle transition seam over create, revise, planning window, pause, resume, delete, supersede pending occurrences, sync target snapshot for active or paused materialized session bindings, revision, and planning cursor rules

### Scenarios
- `schedule_pause_resume_delete` — schedule transitions through create, pause, resume, and delete while advancing revision
- `schedule_revision_and_planning` — active or paused schedules revise, record planning windows, sync target snapshots for materialized session bindings, confirm superseded occurrences, supersede pending occurrences, maintain positive revision, and require occurrence progress for planning cursor

### Transitions
- `CreateSchedule`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_pause_resume_delete`
- `ReviseActive`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `RevisePaused`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `RecordPlanningWindowActive`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `SyncTargetSnapshotActive`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `SyncTargetSnapshotPaused`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `PauseActiveOrPaused`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `ResumeActiveOrPaused`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_pause_resume_delete`, `schedule_revision_and_planning`
- `DeleteActive`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_pause_resume_delete`, `schedule_revision_and_planning`
- `DeletePaused`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_pause_resume_delete`, `schedule_revision_and_planning`
- `DeleteDeleted`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_pause_resume_delete`
- `ConfirmOccurrencesSupersededActive`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `ConfirmOccurrencesSupersededPaused`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `ConfirmOccurrencesSupersededDeleted`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`

### Effects
- `EmitScheduleNotice`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_pause_resume_delete`, `schedule_revision_and_planning`
- `SupersedePendingOccurrences`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `PlanningWindowRecorded`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`

### Invariants
- `revision_is_positive`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `deleted_has_no_planning_cursor`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `planning_cursor_requires_occurrence_progress`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`


<!-- GENERATED_COVERAGE_END -->
