

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `schedule_mob_bundle`

### Code Anchors
- `schedule_driver`: `meerkat-schedule/src/driver.rs` — mechanical scheduler driver precursor for mob-target claim, handoff, and feedback
- `mob_delivery_precursor`: `meerkat-mob-mcp/src/lib.rs` — mob-owned action delivery precursor that scheduling must hand off into
- `schedule_mob_bundle_schema`: `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule mob bundle composition

### Scenarios
- `mob-delivery-feedback` — DispatchToMob is realized by mob-owned delivery and closed by typed completion feedback
- `materialization-failure-classification` — mob-side delivery failure preserves explicit TargetMaterializationFailed classification

### Routes
- `revision_supersede_enters_occurrence_authority`
  - anchors: `schedule_driver`, `mob_delivery_precursor`, `schedule_mob_bundle_schema`
  - scenarios: `mob-delivery-feedback`, `materialization-failure-classification`
- `occurrence_supersede_ack_returns_to_schedule`
  - anchors: `schedule_driver`, `mob_delivery_precursor`, `schedule_mob_bundle_schema`
  - scenarios: `mob-delivery-feedback`, `materialization-failure-classification`

### Scheduler Rules
- `(none)`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
