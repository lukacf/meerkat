# ScheduleLifecycleMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `ScheduleLifecycleMachine`

### Code Anchors
- `schedule_authority`: `meerkat-schedule/src/authority.rs` — schedule lifecycle authority that owns revision, pause/resume, and delete semantics
- `schedule_service`: `meerkat-schedule/src/service.rs` — schedule service precursor for revision supersession and rolling planning
- `schedule_schema`: `meerkat-machine-schema/src/catalog/schedule_lifecycle.rs` — formal ScheduleLifecycleMachine schema

### Scenarios
- `schedule-revision-supersede` — revision-affecting updates bump revision and explicitly supersede pending future occurrences
- `schedule-pause-resume` — pause freezes claiming and resume re-enables planning without bumping revision
- `schedule-delete` — delete terminalizes the schedule while preserving occurrence history

### Transitions
- `ReviseActive`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `RevisePaused`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-pause-resume`
- `RecordPlanningWindowActive`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`, `schedule-pause-resume`
- `RecordPlanningWindowPaused`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-pause-resume`
- `PauseActive`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-pause-resume`
- `ResumePaused`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-pause-resume`
- `DeleteActive`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-delete`
- `DeletePaused`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-pause-resume`

### Effects
- `EmitScheduleNotice`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `SupersedePendingOccurrences`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`, `schedule-delete`
- `PlanningWindowRecorded`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`, `schedule-pause-resume`

### Invariants
- `revision_is_positive`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `deleted_has_no_planning_cursor`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-delete`
- `planning_cursor_requires_occurrence_progress`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`


<!-- GENERATED_COVERAGE_END -->
