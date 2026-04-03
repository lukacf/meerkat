# schedule_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `schedule`: `ScheduleLifecycleMachine` @ actor `schedule_authority`
- `occurrence`: `OccurrenceLifecycleMachine` @ actor `occurrence_authority`

## Routes
- `revision_supersede_enters_occurrence_authority`: `schedule`.`SupersedePendingOccurrences` -> `occurrence`.`SupersedeByRevision` [Immediate]

## Target Selectors
- `(none)`

## Driver
- `(none)`

## Transaction Plans
- `transactional_claim` via `claim_due_occurrences` / `ScheduleStore::claim_due_occurrences` — store-backed claim uses authoritative store time plus durable lease state
- `revision_supersede_and_replan` via `update_schedule_revision` / `ScheduleStore::put_schedule + put_occurrences` — revision-affecting schedule updates supersede pending future occurrences before replanning

## Scheduler Rules
- `(none)`

## Structural Requirements
- `schedule_revision_supersede_route_present` — revision-affecting schedule edits enter occurrence authority through the explicit supersede route

## Behavioral Invariants
- `superseded_occurrence_originates_from_schedule_revision` — pending future occurrences are superseded only by the schedule revision route rather than by ad hoc shell mutation

## Coverage
### Code Anchors
- `meerkat-schedule/src/service.rs` — schedule service precursor for revision supersession and rolling planning
- `meerkat-schedule/src/store.rs` — schedule store contract precursor for transactional claim and supersede persistence
- `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule bundle composition

### Scenarios
- `revision-supersede-route` — revision-affecting schedule updates supersede pending future occurrences through the explicit route
- `pause-resume-without-revision` — pause and resume leave schedule revision unchanged while preserving typed ownership
