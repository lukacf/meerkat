# adaptive_mob_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `control_mob`: `MobMachine` @ actor `control_mob_authority`
- `layer_mob`: `MobMachine` @ actor `layer_mob_authority`

## Routes
- `layer_terminal_reaches_adaptive_kernel`: `layer_mob`.`FlowRunPublicResultClassified` -> `control_mob`.`IngestLayerTerminal` [Immediate]

## Target Selectors
- `(none)`

## Driver
- `adaptive_mob_bundle_driver` (`AdaptiveMobBundleDriver` in `meerkat-mob/src/generated/adaptive_mob_bundle.rs`)
  - watches:
    - `layer_mob::FlowRunPublicResultClassified`
  - dispatches:
    - `layer_terminal_reaches_adaptive_kernel` → `control_mob::IngestLayerTerminal` (Input)

## Transaction Plans
- `adaptive_layer_terminal_feedback` via `ingest_layer_terminal` / `AdaptiveMobBundleDriver::ingest_layer_terminal` — adaptive bundle driver enriches a terminal layer-mob result with the stored adaptive run/layer context before feeding the control mob kernel

## Scheduler Rules
- `(none)`

## Structural Requirements
- `layer_terminal_feedback_route_present` — terminal layer-mob public result classification feeds the adaptive control mob through the canonical generated route
- `control_mob_destroying_session_ingress_protocol_covered` — control mob destroy keeps the canonical detach-before-destroy session ingress handoff explicit inside the adaptive bundle
- `layer_mob_destroying_session_ingress_protocol_covered` — layer mob destroy keeps the canonical detach-before-destroy session ingress handoff explicit inside the adaptive bundle

## Behavioral Invariants
- `(none)`

## Coverage
### Code Anchors
- `adaptive_mob_bundle_kernel` (machine `MobMachine`): `meerkat-mob/src/runtime/handle.rs` — adaptive Mobpack control mob owns the adaptive run kernel while layer mobs publish terminal classifications through the driver seam
- `adaptive_mob_bundle_driver` (machine `MobMachine`): `meerkat-mob/src/generated/adaptive_mob_bundle.rs` — generated adaptive bundle driver watches layer terminal classification and dispatches typed terminal feedback into the control mob adaptive kernel

### Scenarios
- `layer-terminal-feedback` — a terminal child layer mob is observed by the adaptive bundle driver and fed back to the control mob adaptive kernel without a direct static route
