# meerkat_mob_seam Mapping Note

`meerkat_mob_seam` is the only inter-kernel composition in the cutover
architecture.

## Primary code anchors

- `meerkat-runtime/src/meerkat_machine.rs` — Meerkat-side runtime/session
  authority
- `meerkat-mob/src/runtime/handle.rs` — Mob-side command submission and
  projection surface
- `meerkat-mob/src/runtime/actor.rs` — Mob-side lifecycle/work orchestration
- `meerkat-mob/src/runtime/session_service.rs` — bridge-facing live session
  integration

## What this composition replaces

- old runtime/mob bundle routing is gone
- internal runtime routes are modeled inside `MeerkatMachine`
- internal mob routes are modeled inside `MobMachine`
- only cross-kernel lifecycle/work coupling remains in composition space

## Retained perimeter compositions

The schedule perimeter compositions remain outside this seam because they route
schedule/occurrence delivery protocols rather than internal kernel ownership:

- `schedule_bundle`
- `schedule_runtime_bundle`
- `schedule_mob_bundle`

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `meerkat_mob_seam`

### Code Anchors
- `mob_meerkat_seam` (route `binding_request_reaches_meerkat`): `meerkat-mob/src/runtime/actor.rs` — MobMachine to MeerkatMachine seam realization for binding requests, work submission, cancellation, lifecycle notices, terminal outcomes, and peer ingress
- `meerkat_runtime_entry` (machine `MeerkatMachine`): `meerkat-runtime/src/meerkat_machine/mod.rs` — MeerkatMachine command authority consuming runtime binding, admitted work, cancellation, lifecycle, terminal, and peer ingress seam traffic

### Scenarios
- `binding_round_trip` — mob runtime binding request becomes a Meerkat binding and feeds readiness back to Mob
- `work_round_trip` — mob submits work into Meerkat and observes terminal work outcomes back across the seam
- `peer-ingress-and-cancellation` — peer input admission and cancellation requests cross the MobMachine to MeerkatMachine seam with explicit lifecycle notice feedback

### Routes
- `binding_request_reaches_meerkat`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `work_request_reaches_meerkat`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `retire_request_reaches_meerkat`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `destroy_request_reaches_meerkat`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `runtime_bound_reaches_mob`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `runtime_retired_reaches_mob`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `runtime_destroyed_reaches_mob`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)

### Scheduler Rules
- `(none)`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
