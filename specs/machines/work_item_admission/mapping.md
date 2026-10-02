# WorkItemAdmissionMachine Mapping Note

<!-- GENERATED_COVERAGE_START -->
## Generated Coverage
This section is generated from the Rust machine catalog. Do not edit it by hand.

### Machine
- `WorkItemAdmissionMachine`

### Code Anchors
- `work_item_admission` (machine `WorkItemAdmissionMachine`): `crates/meerkat-workgraph/src/machine.rs` — WorkItemAdmissionMachine owner of a work item's exact keyed admission identity: BindKeyed and BindUnkeyed record (or decline) the identity delivered by the lifecycle Created route, and ClassifyAdmissionReplayExact, ClassifyAdmissionReplayConflict and ClassifyAdmissionReplayKeyMismatch decide, over the recovered identity, whether a keyed create that found an existing item is an exact replay, a typed conflict, or a store-index mismatch; effects Bound, AdmissionReplayClassified; invariants admitted_has_identity, non_admitted_has_no_identity

### Scenarios
- `work_item_admission_replay` — a keyed create binds its identity once; an exact replay under the same key and digest is Replayed, the same key with another digest is Conflict, and another key (or an unkeyed or never-bound item) is KeyMismatch, in every phase

### Transitions
- `BindKeyed`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`
- `BindUnkeyed`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`
- `ClassifyAdmissionReplayExactUnkeyed`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`
- `ClassifyAdmissionReplayExactAdmitted`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`
- `ClassifyAdmissionReplayConflictUnkeyed`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`
- `ClassifyAdmissionReplayConflictAdmitted`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`
- `ClassifyAdmissionReplayKeyMismatchAbsent`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`
- `ClassifyAdmissionReplayKeyMismatchUnkeyed`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`
- `ClassifyAdmissionReplayKeyMismatchAdmitted`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`

### Effects
- `Bound`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`
- `AdmissionReplayClassified`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`

### Invariants
- `admitted_has_identity`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`
- `non_admitted_has_no_identity`
  - anchors: `work_item_admission`
  - scenarios: `work_item_admission_replay`


<!-- GENERATED_COVERAGE_END -->
