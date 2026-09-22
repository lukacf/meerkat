# schedule_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `schedule`: `ScheduleLifecycleMachine` @ actor `schedule_authority`
- `occurrence`: `OccurrenceLifecycleMachine` @ actor `occurrence_authority`

## Routes
- `revision_supersede_enters_occurrence_authority`: `schedule`.`SupersedePendingOccurrences` -> `occurrence`.`Supersede` [Immediate]
- `occurrence_supersede_ack_returns_to_schedule`: `occurrence`.`OccurrencesSuperseded` -> `schedule`.`ConfirmOccurrencesSuperseded` [Immediate]

## Target Selectors
- `(none)`

## Driver
- `(none)`

## Transaction Plans
- `transactional_claim` via `claim_due_occurrences` / `ScheduleStore::claim_due_occurrences` — store-backed claim uses authoritative store time plus durable lease state
- `revision_supersede_and_replan` via `update_schedule_revision` / `ScheduleStore::commit_schedule_mutation` — revision-affecting schedule updates (including deletion) supersede all outstanding nonterminal occurrences of the schedule from older revisions at commit time, including overdue Pending and in-flight Claimed/Dispatching/AwaitingCompletion, through typed occurrence Supersede and reciprocal OccurrencesSuperseded -> ConfirmOccurrencesSuperseded acknowledgement; supersession does not promise cancellation of already-dispatched external work

## Scheduler Rules
- `(none)`

## Structural Requirements
- `schedule_revision_supersede_route_present` — revision-affecting schedule edits enter occurrence authority through the explicit supersede route
- `occurrence_supersede_ack_route_present` — the occurrence authority's supersede-consumption ack returns to the schedule authority through the reciprocal route so the schedule observes completion

## Behavioral Invariants
- `superseded_occurrence_originates_from_schedule_revision` — observed occurrence.Supersede inputs delivered on revision_supersede_enters_occurrence_authority originate from schedule.SupersedePendingOccurrences; this route-scoped provenance invariant neither proves sweep completeness nor excludes other typed Supersede ingress

## Coverage
### Code Anchors
- `schedule_service` (route `revision_supersede_enters_occurrence_authority`): `meerkat-schedule/src/service.rs` — schedule service precursor for revision supersession, rolling planning, occurrence materialization, pause resume, and delete lifecycle routing
- `schedule_store` (route `revision_supersede_enters_occurrence_authority`): `meerkat-schedule/src/store.rs` — schedule store contract precursor for transactional claim, supersede persistence, occurrence progress, and revision-aware planning cursor updates
- `schedule_bundle_schema` (route `revision_supersede_enters_occurrence_authority`): `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule bundle composition

### Scenarios
- `revision-supersede-route` — revision-affecting schedule updates (including deletion) supersede all outstanding nonterminal occurrences of the schedule from older revisions at commit time, including overdue Pending and in-flight Claimed/Dispatching/AwaitingCompletion, through typed occurrence Supersede and reciprocal OccurrencesSuperseded -> ConfirmOccurrencesSuperseded acknowledgement; supersession does not promise cancellation of already-dispatched external work
- `pause-resume-without-revision` — pause and resume leave schedule revision unchanged while preserving typed ownership
- `rolling-planning-occurrence-materialization` — rolling planning records a planning window and materializes or supersedes pending occurrences through revision-aware schedule routes
