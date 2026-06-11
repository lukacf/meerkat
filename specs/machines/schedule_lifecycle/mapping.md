# ScheduleLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `ScheduleLifecycleMachine`

### Code Anchors
- `schedule_lifecycle` (machine `ScheduleLifecycleMachine`): `meerkat-schedule/src/lifecycle.rs` — Schedule::apply domain-facing lifecycle transition seam over create, revise, update planning config active or paused, planning window, pause, resume, delete, supersede pending occurrences, sync target snapshot for active or paused materialized session bindings, revision, and planning cursor rules

### Scenarios
- `schedule_pause_resume_delete` — schedule transitions through create, pause, resume, and delete while advancing revision
- `schedule_revision_and_planning` — active or paused schedules revise, update planning config active or paused, record planning windows, sync target snapshots for materialized session bindings, confirm superseded occurrences, supersede pending occurrences, maintain positive revision, and require occurrence progress for planning cursor

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
- `UpdatePlanningConfigActive`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `UpdatePlanningConfigPaused`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `RecordPlanningWindowActive`
  - anchors: (unclaimed)
  - scenarios: `schedule_revision_and_planning`
- `SyncTargetSnapshotActive`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `SyncTargetSnapshotPaused`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `PauseActiveOrPaused`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ResumeActiveOrPaused`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `DeleteActive`
  - anchors: `schedule_lifecycle`
  - scenarios: (unclaimed)
- `DeletePaused`
  - anchors: `schedule_lifecycle`
  - scenarios: (unclaimed)
- `DeleteDeleted`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `ConfirmOccurrencesSupersededActive`
  - anchors: (unclaimed)
  - scenarios: `schedule_revision_and_planning`
- `ConfirmOccurrencesSupersededPaused`
  - anchors: (unclaimed)
  - scenarios: `schedule_revision_and_planning`
- `ConfirmOccurrencesSupersededDeleted`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)

### Effects
- `EmitScheduleNotice`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `SupersedePendingOccurrences`
  - anchors: `schedule_lifecycle`
  - scenarios: `schedule_revision_and_planning`
- `PlanningWindowRecorded`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)

### Invariants
- `revision_is_positive`
  - anchors: (unclaimed)
  - scenarios: `schedule_revision_and_planning`
- `deleted_has_no_planning_cursor`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `planning_cursor_requires_occurrence_progress`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)


<!-- GENERATED_COVERAGE_END -->
