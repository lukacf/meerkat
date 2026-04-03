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
  - scenarios: `schedule-revision-supersede`
- `RecordPlanningWindowActive`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `RecordPlanningWindowPaused`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `PauseActive`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `ResumePaused`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `DeleteActive`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `DeletePaused`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`

### Effects
- `EmitScheduleNotice`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `SupersedePendingOccurrences`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `PlanningWindowRecorded`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`

### Invariants
- `revision_is_positive`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `deleted_has_no_planning_cursor`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`
- `planning_cursor_never_exceeds_next_ordinal_fact`
  - anchors: `schedule_authority`, `schedule_service`, `schedule_schema`
  - scenarios: `schedule-revision-supersede`


<!-- GENERATED_COVERAGE_END -->
