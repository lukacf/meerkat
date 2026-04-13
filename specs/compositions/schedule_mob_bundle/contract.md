# schedule_mob_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `occurrence`: `OccurrenceLifecycleMachine` @ actor `occurrence_authority`

## Routes

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
- `meerkat-schedule/src/driver.rs` — mechanical scheduler driver precursor for mob-target claim, handoff, and feedback
- `meerkat-mob-mcp/src/lib.rs` — mob-owned action delivery precursor that scheduling must hand off into
- `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule mob bundle composition

### Scenarios
- `mob-delivery-feedback` — DispatchToMob is realized by mob-owned delivery and closed by typed completion feedback
- `materialization-failure-classification` — mob-side delivery failure preserves explicit TargetMaterializationFailed classification
