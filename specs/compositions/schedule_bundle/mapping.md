# schedule_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `schedule_bundle`

### Code Anchors
- `schedule_service`: `meerkat-schedule/src/service.rs` — schedule service precursor for revision supersession and rolling planning
- `schedule_store`: `meerkat-schedule/src/store.rs` — schedule store contract precursor for transactional claim and supersede persistence
- `schedule_bundle_schema`: `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule bundle composition

### Scenarios
- `revision-supersede-route` — revision-affecting schedule updates supersede pending future occurrences through the explicit route
- `pause-resume-without-revision` — pause and resume leave schedule revision unchanged while preserving typed ownership

### Routes
- `revision_supersede_enters_occurrence_authority`
  - anchors: `schedule_service`, `schedule_store`, `schedule_bundle_schema`
  - scenarios: `revision-supersede-route`

### Scheduler Rules
- `(none)`

### Invariants
- `schedule_revision_supersede_route_present`
  - anchors: `schedule_service`, `schedule_store`, `schedule_bundle_schema`
  - scenarios: `revision-supersede-route`
- `superseded_occurrence_originates_from_schedule_revision`
  - anchors: `schedule_service`, `schedule_store`, `schedule_bundle_schema`
  - scenarios: `revision-supersede-route`


<!-- GENERATED_COVERAGE_END -->
