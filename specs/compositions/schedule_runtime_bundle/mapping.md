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
- `schedule_driver` (route `revision_supersede_enters_occurrence_authority`): `meerkat-schedule/src/driver.rs` — mechanical scheduler driver precursor for runtime-target claim, revision supersede, handoff, lease expiry, delivery failure, and completion feedback
- `runtime_delivery_precursor` (route `revision_supersede_enters_occurrence_authority`): `meerkat-rpc/src/session_runtime.rs` — runtime-owned prompt/event delivery precursor that scheduling must hand off into for dispatch, completion, failure, and lease recovery
- `schedule_runtime_bundle_schema` (route `revision_supersede_enters_occurrence_authority`): `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule runtime bundle composition

### Scenarios
- `runtime-delivery-feedback` — DispatchToRuntime is realized by runtime-owned delivery and closed by typed completion feedback
- `runtime-lease-expiry` — runtime owner fairness still allows lease expiry to return a stuck occurrence to claimable
- `runtime-revision-supersede` — schedule revision supersede enters occurrence authority before runtime handoff so stale pending work is cancelled explicitly

### Routes
- `revision_supersede_enters_occurrence_authority`
  - anchors: `schedule_driver`
  - scenarios: `runtime-revision-supersede`
- `occurrence_supersede_ack_returns_to_schedule`
  - anchors: `schedule_driver`
  - scenarios: `runtime-revision-supersede`

### Scheduler Rules
- `(none)`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
