# adaptive_mob_bundle Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `adaptive_mob_bundle`

### Code Anchors
- `adaptive_mob_bundle_kernel` (machine `MobMachine`): `meerkat-mob/src/runtime/handle.rs` — adaptive Mobpack control mob owns the adaptive run kernel while layer mobs publish terminal classifications through the driver seam
- `adaptive_mob_bundle_driver` (machine `MobMachine`): `meerkat-mob/src/generated/adaptive_mob_bundle.rs` — generated adaptive bundle driver watches layer terminal classification and dispatches typed terminal feedback into the control mob adaptive kernel

### Scenarios
- `layer-terminal-feedback` — a terminal child layer mob is observed by the adaptive bundle driver and fed back to the control mob adaptive kernel without a direct static route

### Routes
- `layer_terminal_reaches_adaptive_kernel`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)

### Scheduler Rules
- `(none)`

### Invariants
- `layer_terminal_feedback_route_present`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `control_mob_destroying_session_ingress_protocol_covered`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)
- `layer_mob_destroying_session_ingress_protocol_covered`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)


<!-- GENERATED_COVERAGE_END -->
