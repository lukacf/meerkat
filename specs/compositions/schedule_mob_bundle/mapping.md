

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust composition catalog. Do not edit it by hand.

### Composition
- `schedule_mob_bundle`

### Code Anchors
- `schedule_driver` (route `revision_supersede_enters_occurrence_authority`): `meerkat-schedule/src/driver.rs` — mechanical scheduler driver precursor for mob-target claim, revision supersede, handoff, lease expiry, delivery failure, and completion feedback
- `mob_delivery_precursor` (route `revision_supersede_enters_occurrence_authority`): `meerkat-mob-mcp/src/lib.rs` — mob-owned action delivery precursor that scheduling must hand off into for dispatch, completion, target materialization failure, and lease recovery
- `schedule_mob_bundle_schema` (route `revision_supersede_enters_occurrence_authority`): `meerkat-machine-schema/src/catalog/compositions.rs` — formal schedule mob bundle composition

### Scenarios
- `mob-delivery-feedback` — DispatchToMob is realized by mob-owned delivery and closed by typed completion feedback
- `materialization-failure-classification` — mob-side delivery failure preserves explicit TargetMaterializationFailed classification
- `mob-revision-supersede` — schedule revision supersede enters occurrence authority before mob handoff so stale pending work is cancelled explicitly

### Routes
- `revision_supersede_enters_occurrence_authority`
  - anchors: (unclaimed)
  - scenarios: `mob-revision-supersede`
- `occurrence_supersede_ack_returns_to_schedule`
  - anchors: (unclaimed)
  - scenarios: (unclaimed)

### Scheduler Rules
- `(none)`

### Invariants
- `(none)`


<!-- GENERATED_COVERAGE_END -->
