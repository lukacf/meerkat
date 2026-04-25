# schedule_runtime_bundle

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
- `transactional_runtime_claim` via `claim_and_runtime_handoff` / `ScheduleStore::claim_due_occurrences` — transactional claim establishes the durable lease before runtime delivery begins

## Scheduler Rules
- `(none)`

## Structural Requirements
- `(none)`

## Behavioral Invariants
- `(none)`

## Coverage
### Code Anchors
- `meerkat-schedule/src/driver.rs` — mechanical scheduler driver precursor for runtime-target claim, revision supersede, handoff, lease expiry, delivery failure, and completion feedback
- `meerkat-rpc/src/session_runtime.rs` — runtime-owned prompt/event delivery precursor that scheduling must hand off into for dispatch, completion, failure, and lease recovery
- `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule runtime bundle composition

### Scenarios
- `runtime-delivery-feedback` — DispatchToRuntime is realized by runtime-owned delivery and closed by typed completion feedback
- `runtime-lease-expiry` — runtime owner fairness still allows lease expiry to return a stuck occurrence to claimable
- `runtime-revision-supersede` — schedule revision supersede enters occurrence authority before runtime handoff so stale pending work is cancelled explicitly
