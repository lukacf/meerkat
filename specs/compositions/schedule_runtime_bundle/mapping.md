# schedule_runtime_bundle Mapping Note

This composition was audited during the two-kernel collapse and retained
unchanged because it does not route through any absorbed Meerkat or Mob
internal machine.

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `schedule_runtime_bundle`

### Code Anchors
- `schedule_driver`: `meerkat-schedule/src/driver.rs` — mechanical scheduler driver precursor for runtime-target claim, handoff, and feedback
- `runtime_delivery_precursor`: `meerkat-rpc/src/session_runtime.rs` — runtime-owned prompt/event delivery precursor that scheduling must hand off into
- `schedule_runtime_bundle_schema`: `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule runtime bundle composition

### Scenarios
- `runtime-delivery-feedback` — DispatchToRuntime is realized by runtime-owned delivery and closed by typed completion feedback
- `runtime-lease-expiry` — runtime owner fairness still allows lease expiry to return a stuck occurrence to claimable

### Routes
- `(none)`

### Scheduler Rules
- `(none)`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
