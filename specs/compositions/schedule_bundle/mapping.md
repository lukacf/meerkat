# schedule_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `schedule_bundle`

### Code Anchors
- `schedule_service`: `meerkat-schedule/src/service.rs` — schedule service precursor for revision supersession, rolling planning, occurrence materialization, pause resume, and delete lifecycle routing
- `schedule_store`: `meerkat-schedule/src/store.rs` — schedule store contract precursor for transactional claim, supersede persistence, occurrence progress, and revision-aware planning cursor updates
- `schedule_bundle_schema`: `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule bundle composition

### Scenarios
- `revision-supersede-route` — revision-affecting schedule updates supersede pending future occurrences through the explicit route
- `pause-resume-without-revision` — pause and resume leave schedule revision unchanged while preserving typed ownership
- `rolling-planning-occurrence-materialization` — rolling planning records a planning window and materializes or supersedes pending occurrences through revision-aware schedule routes

### Routes
- `revision_supersede_enters_occurrence_authority`
  - anchors: `schedule_store`
  - scenarios: `revision-supersede-route`, `rolling-planning-occurrence-materialization`
- `occurrence_supersede_ack_returns_to_schedule`
  - anchors: `schedule_store`
  - scenarios: `revision-supersede-route`, `rolling-planning-occurrence-materialization`

### Scheduler Rules
- `(none)`

### Invariants
- `schedule_revision_supersede_route_present`
  - anchors: `schedule_store`
  - scenarios: `revision-supersede-route`, `rolling-planning-occurrence-materialization`
- `superseded_occurrence_originates_from_schedule_revision`
  - anchors: `schedule_service`, `schedule_store`
  - scenarios: `revision-supersede-route`, `rolling-planning-occurrence-materialization`
- `occurrence_supersede_ack_route_present`
  - anchors: `schedule_store`
  - scenarios: `revision-supersede-route`, `rolling-planning-occurrence-materialization`


<!-- GENERATED_COVERAGE_END -->
