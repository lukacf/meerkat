# schedule_mob_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `occurrence`: `OccurrenceLifecycleMachine` @ actor `occurrence_authority`
- `schedule`: `ScheduleLifecycleMachine` @ actor `schedule_authority`

## Routes
- `revision_supersede_enters_occurrence_authority`: `schedule`.`SupersedePendingOccurrences` -> `occurrence`.`Supersede` [Immediate]
- `occurrence_supersede_ack_returns_to_schedule`: `occurrence`.`OccurrencesSuperseded` -> `schedule`.`ConfirmOccurrencesSuperseded` [Immediate]

## Target Selectors
- `(none)`

## Driver
- `(none)`

## Transaction Plans
- `transactional_mob_claim` via `claim_and_mob_handoff` / `ScheduleStore::claim_due_occurrences` — transactional claim establishes the durable lease before mob delivery begins

## Scheduler Rules
- `(none)`

## Structural Requirements
- `(none)`

## Behavioral Invariants
- `(none)`

## Coverage
### Code Anchors
- `schedule_driver` (route `revision_supersede_enters_occurrence_authority`): `meerkat-schedule/src/driver.rs` — mechanical scheduler driver precursor for mob-target claim, revision supersede, handoff, lease expiry, delivery failure, and completion feedback
- `mob_delivery_precursor` (route `revision_supersede_enters_occurrence_authority`): `meerkat-mob-mcp/src/lib.rs` — mob-owned action delivery precursor that scheduling must hand off into for dispatch, completion, target materialization failure, and lease recovery
- `schedule_mob_bundle_schema` (route `revision_supersede_enters_occurrence_authority`): `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule mob bundle composition

### Scenarios
- `mob-delivery-feedback` — DispatchToMob is realized by mob-owned delivery and closed by typed completion feedback
- `materialization-failure-classification` — mob-side delivery failure preserves explicit TargetMaterializationFailed classification
- `mob-revision-supersede` — schedule revision supersede enters occurrence authority before mob handoff so stale pending work is cancelled explicitly
