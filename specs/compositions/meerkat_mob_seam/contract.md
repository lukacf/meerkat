# meerkat_mob_seam

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `meerkat`: `MeerkatMachine` @ actor `meerkat_kernel`
- `mob`: `MobMachine` @ actor `mob_kernel`

## Routes
- `binding_request_reaches_meerkat`: `mob`.`RequestRuntimeBinding` -> `meerkat`.`PrepareBindings` [Immediate]
- `member_work_reaches_meerkat`: `mob`.`SubmitMemberWork` -> `meerkat`.`SubmitMobWork` [Immediate]
- `retire_request_reaches_meerkat`: `mob`.`RequestRuntimeRetire` -> `meerkat`.`Retire` [Immediate]
- `destroy_request_reaches_meerkat`: `mob`.`RequestRuntimeDestroy` -> `meerkat`.`Destroy` [Immediate]
- `runtime_bound_reaches_mob`: `meerkat`.`RuntimeBound` -> `mob`.`ObserveRuntimeReady` [Immediate]
- `runtime_retired_reaches_mob`: `meerkat`.`RuntimeRetired` -> `mob`.`ObserveRuntimeRetired` [Immediate]
- `runtime_destroyed_reaches_mob`: `meerkat`.`RuntimeDestroyed` -> `mob`.`ObserveRuntimeDestroyed` [Immediate]
- `work_completed_reaches_mob`: `meerkat`.`WorkCompleted` -> `mob`.`ObserveWorkCompleted` [Immediate]
- `work_failed_reaches_mob`: `meerkat`.`WorkFailed` -> `mob`.`ObserveWorkFailed` [Immediate]
- `work_cancelled_reaches_mob`: `meerkat`.`WorkCancelled` -> `mob`.`ObserveWorkCancelled` [Immediate]

## Target Selectors
- `(none)`

## Driver
- `(none)`

## Transaction Plans
- `(none)`

## Scheduler Rules
- `(none)`

## Structural Requirements
- `(none)`

## Behavioral Invariants
- `(none)`

## Coverage
### Code Anchors
- `meerkat-mob/src/runtime/actor.rs` — MobMachine to MeerkatMachine seam realization
- `meerkat-runtime/src/meerkat_machine.rs` — MeerkatMachine command authority consuming seam traffic

### Scenarios
- `binding_round_trip` — mob runtime binding request becomes a Meerkat binding and feeds readiness back to Mob
- `work_round_trip` — mob submits work into Meerkat and observes terminal work outcomes back across the seam
