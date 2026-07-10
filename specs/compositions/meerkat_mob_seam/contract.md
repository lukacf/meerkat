# meerkat_mob_seam

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `meerkat`: `MeerkatMachine` @ actor `meerkat_kernel`
- `mob`: `MobMachine` @ actor `mob_kernel`

## Routes
- `binding_request_reaches_meerkat`: `mob`.`RequestRuntimeBinding` -> `meerkat`.`PrepareBindings` [Immediate]
- `work_request_reaches_meerkat`: `mob`.`RequestRuntimeIngress` -> `meerkat`.`Ingest` [Immediate]
- `retire_request_reaches_meerkat`: `mob`.`RequestRuntimeRetire` -> `meerkat`.`Retire` [Immediate]
- `destroy_request_reaches_meerkat`: `mob`.`RequestRuntimeDestroy` -> `meerkat`.`Destroy` [Immediate]
- `runtime_bound_reaches_mob`: `meerkat`.`RuntimeBound` -> `mob`.`ObserveRuntimeReady` [Immediate]
- `runtime_retired_reaches_mob`: `meerkat`.`RuntimeRetired` -> `mob`.`ObserveRuntimeRetired` [Immediate]
- `runtime_destroyed_reaches_mob`: `meerkat`.`RuntimeDestroyed` -> `mob`.`ObserveRuntimeDestroyed` [Immediate]

## Target Selectors
- `(none)`

## Driver
- `meerkat_mob_seam_driver` (`MeerkatMobSeamDriver` in `meerkat-runtime/src/generated/meerkat_mob_seam.rs`)
  - watches:
    - `mob::RequestRuntimeBinding`
    - `mob::RequestRuntimeIngress`
    - `mob::RequestRuntimeRetire`
    - `mob::RequestRuntimeDestroy`
  - dispatches:
    - `binding_request_reaches_meerkat` → `meerkat::PrepareBindings` (Input)
    - `work_request_reaches_meerkat` → `meerkat::Ingest` (Input)
    - `retire_request_reaches_meerkat` → `meerkat::Retire` (Input)
    - `destroy_request_reaches_meerkat` → `meerkat::Destroy` (Input)
  - consumer-refusal closures:
    - `binding_request_reaches_meerkat` refusal → `mob::ResolveRuntimeBindingRefusal`
    - `work_request_reaches_meerkat` refusal → `mob::ResolveRuntimeIngressRefusal`
    - `retire_request_reaches_meerkat` refusal → `mob::ResolveRuntimeRetireRefusal`

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
- `mob_meerkat_seam` (route `binding_request_reaches_meerkat`): `meerkat-mob/src/runtime/actor.rs` — MobMachine to MeerkatMachine seam realization for binding requests, work submission, cancellation, lifecycle notices, terminal outcomes, and peer ingress
- `meerkat_runtime_entry` (machine `MeerkatMachine`): `meerkat-runtime/src/meerkat_machine/mod.rs` — MeerkatMachine command authority consuming runtime binding, admitted work, cancellation, lifecycle, terminal, and peer ingress seam traffic

### Scenarios
- `binding_round_trip` — mob runtime binding request becomes a Meerkat binding and feeds readiness back to Mob
- `work_round_trip` — mob submits work into Meerkat and observes terminal work outcomes back across the seam
- `peer-ingress-and-cancellation` — peer input admission and cancellation requests cross the MobMachine to MeerkatMachine seam with explicit lifecycle notice feedback
