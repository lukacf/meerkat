# adaptive_mob_bundle

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `control_mob`: `MobMachine` @ actor `control_mob_authority`
- `layer_mob`: `MobMachine` @ actor `layer_mob_authority`

## Routes

## Target Selectors
- `(none)`

## Driver
- `adaptive_mob_bundle_driver` (`AdaptiveMobBundleDriver` in `meerkat-mob/src/generated/adaptive_mob_bundle.rs`)
  - watches:
    - `layer_mob::FlowRunPublicResultClassified`
  - dispatches:
    - `layer_terminal_reaches_adaptive_kernel` → `control_mob::IngestLayerTerminal` (Input)

## Transaction Plans
- `(none)`

## Scheduler Rules
- `(none)`

## Structural Requirements
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
