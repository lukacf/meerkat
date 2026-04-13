# ScheduleLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `ScheduleLifecycleMachine`

### Code Anchors
- `schedule_authority`: `meerkat-schedule/src/authority.rs` — schedule lifecycle authority and revision ownership

### Scenarios
- `schedule_pause_resume_delete` — schedule transitions through create, pause, resume, and delete while advancing revision

### Transitions
- `ReviseActive`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `RevisePaused`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `RecordPlanningWindowActive`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `RecordPlanningWindowPaused`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `PauseActive`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `ResumePaused`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `DeleteActive`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `DeletePaused`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`

### Effects
- `EmitScheduleNotice`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `SupersedePendingOccurrences`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `PlanningWindowRecorded`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`

### Invariants
- `revision_is_positive`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `deleted_has_no_planning_cursor`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`
- `planning_cursor_requires_occurrence_progress`
  - anchors: `schedule_authority`
  - scenarios: `schedule_pause_resume_delete`


<!-- GENERATED_COVERAGE_END -->
